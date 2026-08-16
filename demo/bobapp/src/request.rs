// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Builds a `ComputeRoutesRequest` from CLI arguments, reflectively.
//!
//! Every field and enum value is resolved by *name* against the embedded
//! descriptors (spec 0241 S14).  Nothing here would fail to compile if the
//! schema changed under it; it would fail to run, which is the property being
//! demonstrated.

use anyhow::{anyhow, Context, Result};
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage, Value};

/// `ComputeRoutesRequest`, fully qualified.
pub const REQUEST_TYPE: &str = "google.maps.routing.v2.ComputeRoutesRequest";
/// `ComputeRoutesResponse`, fully qualified.
pub const RESPONSE_TYPE: &str = "google.maps.routing.v2.ComputeRoutesResponse";

/// The method bobapp resolves a place name with.
pub const LOOKUP_METHOD: &str = "/google.maps.places.v1.Places/SearchText";
/// `SearchTextRequest`, fully qualified.
pub const LOOKUP_TYPE: &str = "google.maps.places.v1.SearchTextRequest";
/// `SearchTextResponse`, fully qualified.
pub const LOOKUP_RESPONSE_TYPE: &str = "google.maps.places.v1.SearchTextResponse";

/// What the CLI collected, before any of it has met the schema.
pub struct RouteQuery<'a> {
    pub origin: &'a str,
    pub destination: &'a str,
    pub travel_mode: &'a str,
    pub routing_preference: &'a str,
    pub units: &'a str,
    pub language_code: &'a str,
    /// Seconds from now, filling `departure_time` with a `Timestamp`.
    pub depart_in: Option<u64>,
}

/// Resolves a message type, naming what was missing if it is not there.
pub fn message(pool: &DescriptorPool, fqdn: &str) -> Result<MessageDescriptor> {
    pool.get_message_by_name(fqdn)
        .ok_or_else(|| anyhow!("the embedded descriptor set does not define {fqdn}"))
}

/// Builds the request message.
pub fn build(pool: &DescriptorPool, query: &RouteQuery<'_>) -> Result<DynamicMessage> {
    let descriptor = message(pool, REQUEST_TYPE)?;
    let mut request = DynamicMessage::new(descriptor);

    set(
        &mut request,
        "origin",
        Value::Message(waypoint(pool, query.origin)?),
    )?;
    set(
        &mut request,
        "destination",
        Value::Message(waypoint(pool, query.destination)?),
    )?;
    set_enum(&mut request, "travel_mode", query.travel_mode)?;
    set_enum(&mut request, "routing_preference", query.routing_preference)?;
    set_enum(&mut request, "units", query.units)?;
    set(
        &mut request,
        "language_code",
        Value::String(query.language_code.to_owned()),
    )?;

    if let Some(seconds) = query.depart_in {
        set(
            &mut request,
            "departure_time",
            Value::Message(timestamp(pool, seconds)?),
        )?;
    }

    Ok(request)
}

/// Builds a `SearchTextRequest`.
///
/// `pool` is *not* the embedded pool: this build compiled in no Places
/// service, so the caller reads a descriptor set off disk for it.  That is
/// the whole difference between these entries and the routing ones — the
/// schema recovered from the executable names one pair and not the other.
/// `near` is one end of the trip, and biases the search when it is a
/// coordinate pair.
pub fn lookup(
    pool: &DescriptorPool,
    text: &str,
    near: &str,
    language_code: &str,
) -> Result<DynamicMessage> {
    /// How far around `near` the search is biased, in meters.
    const RADIUS: f64 = 1500.0;

    let mut request = DynamicMessage::new(message(pool, LOOKUP_TYPE)?);
    set(&mut request, "text_query", Value::String(text.to_owned()))?;
    set(
        &mut request,
        "language_code",
        Value::String(language_code.to_owned()),
    )?;
    set_enum(&mut request, "rank_preference", "RELEVANCE")?;
    // Deliberately not `open_now`: it would make the number of results — and
    // so the size of the logged response — depend on the hour the artifact
    // was minted, and a fixture minted at midnight comes back all but empty.
    set(&mut request, "min_rating", Value::F64(4.0))?;
    set(&mut request, "max_result_count", Value::I32(5))?;

    if let Some((lat, lng)) = coordinates(near) {
        let mut circle = DynamicMessage::new(message(pool, "google.maps.places.v1.Circle")?);
        set(
            &mut circle,
            "center",
            Value::Message(lat_lng(pool, lat, lng)?),
        )?;
        set(&mut circle, "radius", Value::F64(RADIUS))?;

        let mut bias = DynamicMessage::new(message(
            pool,
            "google.maps.places.v1.SearchTextRequest.LocationBias",
        )?);
        set(&mut bias, "circle", Value::Message(circle))?;
        set(&mut request, "location_bias", Value::Message(bias))?;
    }

    Ok(request)
}

/// A `Waypoint`, taking whichever arm of its `location_type` oneof the caller
/// named the place with: a coordinate pair goes in `location`, anything else
/// in `address`.
fn waypoint(pool: &DescriptorPool, place: &str) -> Result<DynamicMessage> {
    let mut waypoint = DynamicMessage::new(message(pool, "google.maps.routing.v2.Waypoint")?);
    match coordinates(place) {
        Some((lat, lng)) => set(
            &mut waypoint,
            "location",
            Value::Message(location(pool, lat, lng)?),
        )?,
        None => set(&mut waypoint, "address", Value::String(place.to_owned()))?,
    }
    Ok(waypoint)
}

/// `"45.188529, 5.724524"` is a point; `"Grenoble, France"` is not.
///
/// Both halves have to parse, so a place name containing a comma stays a place
/// name.
fn coordinates(place: &str) -> Option<(f64, f64)> {
    let (lat, lng) = place.split_once(',')?;
    Some((lat.trim().parse().ok()?, lng.trim().parse().ok()?))
}

/// A `Location` around a `google.type.LatLng`.
fn location(pool: &DescriptorPool, lat: f64, lng: f64) -> Result<DynamicMessage> {
    let mut location = DynamicMessage::new(message(pool, "google.maps.routing.v2.Location")?);
    set(
        &mut location,
        "lat_lng",
        Value::Message(lat_lng(pool, lat, lng)?),
    )?;
    Ok(location)
}

/// A `google.type.LatLng`.
fn lat_lng(pool: &DescriptorPool, lat: f64, lng: f64) -> Result<DynamicMessage> {
    let mut point = DynamicMessage::new(message(pool, "google.type.LatLng")?);
    set(&mut point, "latitude", Value::F64(lat))?;
    set(&mut point, "longitude", Value::F64(lng))?;
    Ok(point)
}

/// A `google.protobuf.Timestamp` `seconds` from now.
fn timestamp(pool: &DescriptorPool, seconds_from_now: u64) -> Result<DynamicMessage> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("the system clock is before the epoch")?;
    let at = now.as_secs() + seconds_from_now;

    let mut timestamp = DynamicMessage::new(message(pool, "google.protobuf.Timestamp")?);
    set(&mut timestamp, "seconds", Value::I64(at as i64))?;
    Ok(timestamp)
}

/// Sets a field by name, failing if the schema has no such field.
///
/// `DynamicMessage::set_field_by_name` silently does nothing when the name is
/// unknown, which would turn a typo into a request that is quietly missing a
/// field.  Resolving the descriptor first keeps the run-time-by-name property
/// and makes the mistake loud.
pub fn set(message: &mut DynamicMessage, name: &str, value: Value) -> Result<()> {
    let field = message
        .descriptor()
        .get_field_by_name(name)
        .ok_or_else(|| anyhow!("{} has no field {name}", message.descriptor().full_name()))?;
    message.set_field(&field, value);
    Ok(())
}

/// Sets an enum field from the *name* of one of its values.
fn set_enum(message: &mut DynamicMessage, field_name: &str, value_name: &str) -> Result<()> {
    let descriptor = message.descriptor();
    let field = descriptor
        .get_field_by_name(field_name)
        .ok_or_else(|| anyhow!("{} has no field {field_name}", descriptor.full_name()))?;
    let enum_descriptor = field
        .kind()
        .as_enum()
        .cloned()
        .ok_or_else(|| anyhow!("{field_name} is not an enum"))?;
    let value = enum_descriptor
        .get_value_by_name(value_name)
        .ok_or_else(|| {
            let known: Vec<_> = enum_descriptor
                .values()
                .map(|v| v.name().to_owned())
                .collect();
            anyhow!(
                "{value_name} is not a value of {}; known values: {}",
                enum_descriptor.full_name(),
                known.join(", ")
            )
        })?;
    message.set_field(&field, Value::EnumNumber(value.number()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which arm a waypoint takes is the difference between a request only
    /// `routing.v2` can describe and one that `routes.v1` describes just as
    /// well — v1's `Waypoint` has no `address`, but both have `location`.
    /// The demo turns on that, so it is asserted here.
    #[test]
    fn a_coordinate_pair_is_a_point_and_a_place_name_is_not() {
        assert_eq!(
            coordinates("45.188529, 5.724524"),
            Some((45.188529, 5.724524))
        );
        assert_eq!(
            coordinates("45.188529,5.724524"),
            Some((45.188529, 5.724524))
        );
        assert_eq!(coordinates("Grenoble, France"), None);
        assert_eq!(coordinates("Lyon"), None);
        // Half a pair is not a pair.
        assert_eq!(coordinates("45.188529, north"), None);
    }
}
