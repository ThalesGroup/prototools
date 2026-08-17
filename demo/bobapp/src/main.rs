// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! bobapp — a toy gRPC client that leaves bytes worth opening.
//!
//! Calls `google.maps.routing.v2.Routes/ComputeRoutes` reflectively, against
//! descriptors embedded in this executable, and logs the exact bytes it put on
//! the wire so that protolens can open them afterwards.  Spec 0241.

mod anomaly;
mod codec;
mod log;
mod request;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use prost_reflect::DescriptorPool;
use serde::Serialize;
use tonic::{
    transport::{Channel, ClientTlsConfig},
    Request,
};

use crate::{
    codec::DynamicCodec,
    log::Recorder,
    request::{RouteQuery, RESPONSE_TYPE},
};

/// The transitive closure of `routes_service.proto`, put here by `build.rs`
/// from `BOBAPP_DESCRIPTOR_SET` (spec 0241 S5/S8).
const DESCRIPTOR_SET: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bobapp.desc"));

/// Where the route call goes (S11).
const ENDPOINT: &str = "https://routes.googleapis.com";
const METHOD: &str = "/google.maps.routing.v2.Routes/ComputeRoutes";

/// Where a place lookup goes.
const LOOKUP_ENDPOINT: &str = "https://places.googleapis.com";

/// Routes rejects a call without a field mask, so there is a default.
const DEFAULT_FIELD_MASK: &str =
    "routes.duration,routes.distanceMeters,routes.polyline.encodedPolyline,routes.legs.steps";

/// Places rejects one too, and it is not worth a flag of its own.
const LOOKUP_FIELD_MASK: &str =
    "places.displayName,places.formattedAddress,places.location,places.rating";

/// The environment variable holding the API key.
///
/// Read from the environment and from nowhere else (S12) — never a flag, so it
/// cannot reach shell history or `/proc/<pid>/cmdline`.
const API_KEY_VAR: &str = "BOBAPP_API_KEY";

#[derive(Parser, Debug)]
#[command(
    name = "bobapp",
    version,
    about = "Calls a live Google API reflectively and logs the bytes it sent"
)]
struct Cli {
    /// Street address or place name to start from.
    #[arg(long, required_unless_present = "dump_descriptor")]
    origin: Option<String>,

    /// Street address or place name to end at.
    #[arg(long, required_unless_present = "dump_descriptor")]
    destination: Option<String>,

    /// A value of google.maps.routing.v2.RouteTravelMode.
    #[arg(long, default_value = "DRIVE")]
    travel_mode: String,

    /// A value of google.maps.routing.v2.RoutingPreference.
    #[arg(long, default_value = "TRAFFIC_AWARE")]
    routing_preference: String,

    /// A value of google.maps.routing.v2.Units.
    #[arg(long, default_value = "METRIC")]
    units: String,

    /// BCP-47 language tag for the response.
    #[arg(long, default_value = "en-US")]
    language_code: String,

    /// Depart this many seconds from now, filling departure_time.
    #[arg(long)]
    depart_in: Option<u64>,

    /// Value of the x-goog-fieldmask header.
    #[arg(long, default_value = DEFAULT_FIELD_MASK)]
    field_mask: String,

    /// Directory to write log.pb into.
    #[arg(long)]
    log_dir: Option<PathBuf>,

    /// Write the embedded descriptor set to this path and exit.
    #[arg(long)]
    dump_descriptor: Option<PathBuf>,

    /// A descriptor set to read at run time, for services this build did not
    /// compile in.
    #[arg(long)]
    extra_descriptor_set: Option<PathBuf>,

    /// Look a place up by name before routing.  Repeatable.
    ///
    /// The bobapp1 build compiled in no Places service, so there it needs
    /// `--extra-descriptor-set`; bobapp2 has one and does not.  Which is why
    /// clap does not require the flag: whether it is needed is a fact about
    /// the descriptor set this build embeds, and the error, if it comes,
    /// names the type that could not be found.
    #[arg(long)]
    look_up: Vec<String>,
}

/// The pool, built once (S8).
fn pool() -> Result<&'static DescriptorPool> {
    static POOL: OnceLock<Result<DescriptorPool, prost_reflect::DescriptorError>> = OnceLock::new();
    POOL.get_or_init(|| DescriptorPool::decode(DESCRIPTOR_SET))
        .as_ref()
        .map_err(|e| anyhow!("the embedded descriptor set does not parse: {e}"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Before any network setup (S9): the audience extracts the schema from the
    // binary and feeds it straight to `reproto --schema-db-out`.
    if let Some(path) = &cli.dump_descriptor {
        log::write_to(path, DESCRIPTOR_SET)?;
        println!("wrote {} ({} bytes)", path.display(), DESCRIPTOR_SET.len());
        return Ok(());
    }

    let pool = pool()?;
    let api_key = std::env::var(API_KEY_VAR)
        .map_err(|_| anyhow!("{API_KEY_VAR} is not set; bobapp will not call without a key"))?;

    let there = cli.origin.as_deref().expect("required by clap");
    let back = cli.destination.as_deref().expect("required by clap");
    let query = |origin, destination| RouteQuery {
        origin,
        destination,
        travel_mode: &cli.travel_mode,
        routing_preference: &cli.routing_preference,
        units: &cli.units,
        language_code: &cli.language_code,
        depart_in: cli.depart_in,
    };

    let recorder = Arc::new(Mutex::new(Recorder::default()));

    // Names are resolved before they are routed between, so the lookups come
    // first in the log.  Not fatal: a lookup that fails still leaves its
    // request in the log, and the route below is the job.
    if let Err(e) = look_up(&cli, &api_key, &recorder).await {
        eprintln!("a lookup failed: {e:#}");
    }

    let routes = Wire {
        endpoint: ENDPOINT,
        method: METHOD,
        field_mask: &cli.field_mask,
        response: request::message(pool, RESPONSE_TYPE)?,
    };

    // Both directions, because bobapp is a round-trip planner and because the
    // log needs more than one entry to *have* a tail: `write_log` cuts the
    // file short, and with a single entry the cut would land in the outermost
    // record and take the whole document with it.
    let outcome = call(
        &routes,
        &api_key,
        request::build(pool, &query(there, back))?,
        Arc::clone(&recorder),
    )
    .await;
    if outcome.is_ok() {
        // Not fatal, and deliberately not propagated: the caller already has
        // an answer, and the log below must be written either way.
        if let Err(e) = call(
            &routes,
            &api_key,
            request::build(pool, &query(back, there))?,
            Arc::clone(&recorder),
        )
        .await
        {
            eprintln!("the return leg failed: {e:#}");
        }
    }

    // Written whether or not the call succeeded: a failed call is exactly the
    // one whose bytes are worth opening.
    if let Some(dir) = &cli.log_dir {
        write_log(dir, &recorder, pool, &api_key)?;
    }

    let response = outcome?;
    let mut json = serde_json::Serializer::pretty(std::io::stdout());
    response
        .serialize(&mut json)
        .context("rendering the response as JSON")?;
    println!();
    Ok(())
}

/// Calls `SearchText` once per `--look-up`.
///
/// Where the schema comes from is the whole point of shipping two builds.
/// bobapp1 compiled in no Places service, so `--dump-descriptor` cannot
/// produce a schema that names these bytes and the call needs
/// `--extra-descriptor-set` to be made at all — which is exactly what makes
/// the bytes worth opening with a bigger dictionary.  bobapp2 embeds Places,
/// so it needs no flag and its recovered schema reads its own lookups back.
/// The call itself is as real as the routing one either way — same codec,
/// same recorder — so the difference between the two pairs of entries is
/// entirely a matter of which schema can read them.
async fn look_up(cli: &Cli, api_key: &str, recorder: &codec::SharedRecorder) -> Result<()> {
    if cli.look_up.is_empty() {
        return Ok(());
    }
    // Cheap to clone: a DescriptorPool is reference-counted internally.
    let pool = match cli.extra_descriptor_set.as_deref() {
        Some(path) => {
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            DescriptorPool::decode(&bytes[..])
                .with_context(|| format!("{} does not parse as a descriptor set", path.display()))?
        }
        None => pool()?.clone(),
    };

    let wire = Wire {
        endpoint: LOOKUP_ENDPOINT,
        method: request::LOOKUP_METHOD,
        field_mask: LOOKUP_FIELD_MASK,
        response: request::message(&pool, request::LOOKUP_RESPONSE_TYPE)?,
    };

    // Each lookup is biased to one end of the trip, taken in turn: the first
    // is near where it starts, the second near where it ends.
    let ends = [
        cli.origin.as_deref().expect("required by clap"),
        cli.destination.as_deref().expect("required by clap"),
    ];

    for (i, text) in cli.look_up.iter().enumerate() {
        let message = request::lookup(&pool, text, ends[i % ends.len()], &cli.language_code)?;
        call(&wire, api_key, message, Arc::clone(recorder))
            .await
            .with_context(|| format!("looking up {text:?}"))?;
    }
    Ok(())
}

/// One reflective unary call: where it goes, and what comes back.
struct Wire<'a> {
    endpoint: &'static str,
    method: &'static str,
    field_mask: &'a str,
    response: prost_reflect::MessageDescriptor,
}

/// Makes the call, recording both directions through the codec.
async fn call(
    wire: &Wire<'_>,
    api_key: &str,
    message: prost_reflect::DynamicMessage,
    recorder: codec::SharedRecorder,
) -> Result<prost_reflect::DynamicMessage> {
    let channel = Channel::from_static(wire.endpoint)
        .tls_config(ClientTlsConfig::new().with_native_roots())
        .context("configuring TLS")?
        .connect()
        .await
        .with_context(|| format!("connecting to {}", wire.endpoint))?;

    let mut request = Request::new(message);
    request.metadata_mut().insert(
        "x-goog-api-key",
        api_key
            .parse()
            .context("the API key is not a valid header")?,
    );
    request.metadata_mut().insert(
        "x-goog-fieldmask",
        wire.field_mask
            .parse()
            .context("the field mask is not a valid header")?,
    );

    let codec = DynamicCodec::new(wire.method, wire.response.clone(), recorder);

    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready().await.context("the channel never got ready")?;
    let response = grpc
        .unary(
            request,
            wire.method.parse().expect("a valid method path"),
            codec,
        )
        .await
        .context("the call failed")?;

    Ok(response.into_inner())
}

/// Refuses a log that holds the live key.
///
/// This should never fire: the key travels as an `x-goog-api-key` header and
/// [`log::Recorder`] only ever sees message bodies, so there is no path from
/// one to the other.  It exists because "should never" is not "cannot" — the
/// day something starts recording metadata, or a status body, this is what
/// catches it before the bytes reach a file that gets committed.
///
/// The synthetic key [`anomaly`] writes is a different string and is
/// deliberately left alone; it is the anomaly, not a leak.
fn refuse_the_live_key(bytes: &[u8], api_key: &str) -> Result<()> {
    if api_key.is_empty() || bytes.len() < api_key.len() {
        return Ok(());
    }
    if bytes
        .windows(api_key.len())
        .any(|w| w == api_key.as_bytes())
    {
        bail!("the log holds the live API key — refusing to write it");
    }
    Ok(())
}

/// Writes `DIR/log.pb` and prints the command that reads it back (S17).
fn write_log(
    dir: &Path,
    recorder: &codec::SharedRecorder,
    pool: &DescriptorPool,
    api_key: &str,
) -> Result<()> {
    let recorder = recorder.lock().expect("recorder mutex");
    if recorder.is_empty() {
        return Ok(());
    }

    let path = dir.join("log.pb");
    let bytes = recorder.encode_log(pool)?;
    // Before the cut, so that a key sitting in the bytes about to be dropped
    // is still an error rather than a near miss nobody hears about.
    refuse_the_live_key(&bytes, api_key)?;
    // bobapp does not finish writing this file.  Whatever it is that kills it
    // — and the demo never says — the last record on disk is shorter than its
    // own length header promises.
    let bytes = anomaly::cut_short(&bytes);
    log::write_to(&path, bytes)?;

    eprintln!("wrote {} ({} bytes)", path.display(), bytes.len());
    eprintln!(
        "  {} files in the embedded descriptor set, {} bytes",
        pool.files().len(),
        DESCRIPTOR_SET.len()
    );
    eprintln!();
    // Deliberately not "the sweep names the message": the tail this file ends
    // on vetoes every candidate for the document as a whole, so it opens with
    // no type at all.  What names the entries is the cue on one of them.
    eprintln!("Read it back — it opens untyped; the cues name the entries:");
    eprintln!();
    eprintln!("  bobapp --dump-descriptor /tmp/bobapp.desc");
    eprintln!("  reproto --schema-db-out /tmp/bobapp-db /tmp/bobapp.desc");
    eprintln!(
        "  protolens --descriptor-set /tmp/bobapp-db/bobapp.desc {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_live_key_is_refused_and_the_synthetic_one_is_not() {
        let live = "AIzaSyLiveLiveLiveLiveLiveLiveLiveLiveLi";
        let mut log = b"\x1a\x37x-goog-api-key: ".to_vec();
        log.extend_from_slice(live.as_bytes());
        assert!(refuse_the_live_key(&log, live).is_err());

        // The anomaly's key is a different string, and stays.
        assert!(refuse_the_live_key(&log, "AIzaSyB0b5REKn0tAr3aLk3yD0ntB0th3rTry1t").is_ok());
        // A log shorter than the key cannot hold it.
        assert!(refuse_the_live_key(b"\x08\x01", live).is_ok());
    }
}
