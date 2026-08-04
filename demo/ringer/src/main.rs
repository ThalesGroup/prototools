// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! ringer — a toy gRPC client that leaves bytes worth opening.
//!
//! Calls `google.maps.routing.v2.Routes/ComputeRoutes` reflectively, against
//! descriptors embedded in this executable, and logs the exact bytes it put on
//! the wire so that protolens can open them afterwards.  Spec 0241.

mod codec;
mod log;
mod request;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{anyhow, Context, Result};
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
/// from `RINGER_DESCRIPTOR_SET` (spec 0241 S5/S8).
const DESCRIPTOR_SET: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ringer.desc"));

/// Where the call goes (S11).
const ENDPOINT: &str = "https://routes.googleapis.com";
const METHOD: &str = "/google.maps.routing.v2.Routes/ComputeRoutes";

/// Routes rejects a call without a field mask, so there is a default.
const DEFAULT_FIELD_MASK: &str =
    "routes.duration,routes.distanceMeters,routes.polyline.encodedPolyline,routes.legs.steps";

/// The environment variable holding the API key.
///
/// Read from the environment and from nowhere else (S12) — never a flag, so it
/// cannot reach shell history or `/proc/<pid>/cmdline`.
const API_KEY_VAR: &str = "RINGER_API_KEY";

#[derive(Parser, Debug)]
#[command(
    name = "ringer",
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
        .map_err(|_| anyhow!("{API_KEY_VAR} is not set; ringer will not call without a key"))?;

    let query = RouteQuery {
        origin: cli.origin.as_deref().expect("required by clap"),
        destination: cli.destination.as_deref().expect("required by clap"),
        travel_mode: &cli.travel_mode,
        routing_preference: &cli.routing_preference,
        units: &cli.units,
        language_code: &cli.language_code,
        depart_in: cli.depart_in,
    };
    let message = request::build(pool, &query)?;

    let recorder = Arc::new(Mutex::new(Recorder::default()));
    let outcome = call(&cli, &api_key, message, Arc::clone(&recorder)).await;

    // Written whether or not the call succeeded: a failed call is exactly the
    // one whose bytes are worth opening.
    if let Some(dir) = &cli.log_dir {
        write_log(dir, &recorder, pool)?;
    }

    let response = outcome?;
    let mut json = serde_json::Serializer::pretty(std::io::stdout());
    response
        .serialize(&mut json)
        .context("rendering the response as JSON")?;
    println!();
    Ok(())
}

/// Makes the call, recording both directions through the codec.
async fn call(
    cli: &Cli,
    api_key: &str,
    message: prost_reflect::DynamicMessage,
    recorder: codec::SharedRecorder,
) -> Result<prost_reflect::DynamicMessage> {
    let pool = pool()?;
    let channel = Channel::from_static(ENDPOINT)
        .tls_config(ClientTlsConfig::new().with_native_roots())
        .context("configuring TLS")?
        .connect()
        .await
        .with_context(|| format!("connecting to {ENDPOINT}"))?;

    let mut request = Request::new(message);
    request.metadata_mut().insert(
        "x-goog-api-key",
        api_key
            .parse()
            .context("the API key is not a valid header")?,
    );
    request.metadata_mut().insert(
        "x-goog-fieldmask",
        cli.field_mask
            .parse()
            .context("the field mask is not a valid header")?,
    );

    let codec = DynamicCodec::new(request::message(pool, RESPONSE_TYPE)?, recorder);

    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready().await.context("the channel never got ready")?;
    let response = grpc
        .unary(request, METHOD.parse().expect("a valid method path"), codec)
        .await
        .context("the call failed")?;

    Ok(response.into_inner())
}

/// Writes `DIR/log.pb` and prints the command that reads it back (S17).
fn write_log(dir: &Path, recorder: &codec::SharedRecorder, pool: &DescriptorPool) -> Result<()> {
    let recorder = recorder.lock().expect("recorder mutex");
    if recorder.is_empty() {
        return Ok(());
    }

    let path = dir.join("log.pb");
    let bytes = recorder.encode_log();
    log::write_to(&path, &bytes)?;

    eprintln!("wrote {} ({} bytes)", path.display(), bytes.len());
    eprintln!(
        "  {} files in the embedded descriptor set, {} bytes",
        pool.files().len(),
        DESCRIPTOR_SET.len()
    );
    eprintln!();
    eprintln!("Read it back — no --type, the sweep names the message:");
    eprintln!();
    eprintln!("  ringer --dump-descriptor /tmp/ringer.desc");
    eprintln!("  reproto --schema-db-out /tmp/ringer-db /tmp/ringer.desc");
    eprintln!(
        "  protolens --descriptor-set /tmp/ringer-db/ringer.desc {}",
        path.display()
    );
    Ok(())
}
