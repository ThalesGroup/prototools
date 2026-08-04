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

/// A `Waypoint` whose `location_type` oneof takes its `address` arm.
fn waypoint(pool: &DescriptorPool, address: &str) -> Result<DynamicMessage> {
    let mut waypoint = DynamicMessage::new(message(pool, "google.maps.routing.v2.Waypoint")?);
    set(&mut waypoint, "address", Value::String(address.to_owned()))?;
    Ok(waypoint)
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
fn set(message: &mut DynamicMessage, name: &str, value: Value) -> Result<()> {
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
