//! `placements.bin`: the components of a large assembly as fixed-size records.
//!
//! An assembly with more than [`BULK_THRESHOLD`] components keeps every component that
//! has nothing beyond its record (no name) here instead of in `assembly.json`: an
//! imported assembly with a hundred thousand individually placed parts would otherwise
//! be a hundred megabytes of JSON. Named components stay in the JSON. Which components
//! go where depends only on the components, so the same assembly always writes the same
//! bytes; in memory they are one list in id order.
//!
//! The entry, little-endian:
//!
//! | Bytes | Field |
//! |---|---|
//! | 4 | magic `DWPL` |
//! | 4 | version, `u32` 1 |
//! | 4 | record count, `u32` |
//! | 4 | record size in bytes, `u32` 72 |
//! | 72 each | records |
//!
//! A record: component id `u32`, source id `u32`, flags `u32` (bit 0 hidden, bit 1
//! suppressed), zero `u32`, translation `3 x f64` in millimetres, rotation quaternion
//! `4 x f64` (x, y, z, w).

use glam::{DQuat, DVec3};

use crate::binary::{BinaryError, Reader, Writer};
use crate::path::{ComponentId, SourceId};
use crate::placement::Placement;
use crate::schema::Component;

/// The entry name.
pub const PLACEMENTS: &str = "placements.bin";
/// Above this many components, unnamed ones are written to `placements.bin`.
pub const BULK_THRESHOLD: usize = 1024;

const MAGIC: &[u8; 4] = b"DWPL";
const VERSION: u32 = 1;
const RECORD: u32 = 72;
const HIDDEN: u32 = 1;
const SUPPRESSED: u32 = 2;

/// True when `c` is written to `placements.bin` in an assembly of `total` components.
pub fn goes_to_bulk(c: &Component, total: usize) -> bool {
    total > BULK_THRESHOLD && c.name.is_none()
}

/// The entry's bytes for `components`, in the order given. With `geometry` the hidden
/// flag is left out, for the geometry hash.
pub fn encode<'a>(components: impl ExactSizeIterator<Item = &'a Component>, geometry: bool) -> Vec<u8> {
    let mut w = Writer::with_capacity(16 + components.len() * RECORD as usize);
    w.raw(MAGIC);
    w.u32(VERSION);
    w.u32(components.len() as u32);
    w.u32(RECORD);
    for c in components {
        w.u32(c.id.0);
        w.u32(c.source.0);
        let hidden = if c.hidden && !geometry { HIDDEN } else { 0 };
        w.u32(hidden | if c.suppressed { SUPPRESSED } else { 0 });
        w.u32(0);
        for v in c.placement.translation.to_array() {
            w.f64(v);
        }
        for v in c.placement.rotation.to_array() {
            w.f64(v);
        }
    }
    w.bytes
}

/// The components in an entry's bytes.
pub fn decode(bytes: &[u8]) -> Result<Vec<Component>, BinaryError> {
    let bad = |what: &str| BinaryError { entry: PLACEMENTS, what: what.to_owned() };
    let mut r = Reader::new(bytes);
    if r.take(4) != Some(MAGIC.as_slice()) {
        return Err(bad("it does not start with DWPL"));
    }
    match r.u32() {
        Some(VERSION) => {}
        Some(v) => return Err(bad(&format!("version {v} needs a newer SolidDraft"))),
        None => return Err(bad("cut short")),
    }
    let count = r.u32().ok_or_else(|| bad("cut short"))? as usize;
    let size = r.u32().ok_or_else(|| bad("cut short"))?;
    if size < RECORD {
        return Err(bad(&format!("records of {size} bytes are too small")));
    }
    if r.remaining() != count * size as usize {
        return Err(bad("its length does not match its record count"));
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let rec = r.take(size as usize).expect("length checked");
        let mut f = Reader::new(rec);
        let (id, source, flags) = (f.u32().unwrap(), f.u32().unwrap(), f.u32().unwrap());
        f.u32();
        let t = [f.f64().unwrap(), f.f64().unwrap(), f.f64().unwrap()];
        let q = [f.f64().unwrap(), f.f64().unwrap(), f.f64().unwrap(), f.f64().unwrap()];
        if !t.iter().chain(&q).all(|v| v.is_finite()) || DQuat::from_array(q).length() < 1e-12 {
            return Err(bad(&format!("component {id} has an invalid placement")));
        }
        let rotation = DQuat::from_array(q);
        let rotation = if (rotation.length() - 1.0).abs() < 1e-12 { rotation } else { rotation.normalize() };
        out.push(Component {
            id: ComponentId(id),
            name: None,
            source: SourceId(source),
            placement: Placement { rotation, translation: DVec3::from_array(t) },
            hidden: flags & HIDDEN != 0,
            suppressed: flags & SUPPRESSED != 0,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip() {
        let mut a = Component::new(
            ComponentId(7),
            SourceId(2),
            Placement::new(DQuat::from_rotation_y(0.25), DVec3::new(1.0, -2.0, 3.5)),
        );
        a.hidden = true;
        let mut b = Component::new(ComponentId(9), SourceId(2), Placement::IDENTITY);
        b.suppressed = true;
        let bytes = encode([&a, &b].into_iter(), false);
        assert_eq!(bytes.len(), 16 + 2 * 72);
        assert_eq!(decode(&bytes).unwrap(), vec![a.clone(), b.clone()]);
        let geometry = decode(&encode([&a, &b].into_iter(), true)).unwrap();
        assert!(!geometry[0].hidden && geometry[1].suppressed, "the geometry form drops only hidden");
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        assert!(decode(b"nope").is_err());
    }

    #[test]
    fn only_unnamed_components_of_large_assemblies_go_to_bulk() {
        let mut c = Component::new(ComponentId(1), SourceId(1), Placement::IDENTITY);
        assert!(!goes_to_bulk(&c, BULK_THRESHOLD));
        assert!(goes_to_bulk(&c, BULK_THRESHOLD + 1));
        c.name = Some("Main shaft".into());
        assert!(!goes_to_bulk(&c, BULK_THRESHOLD + 1));
    }
}
