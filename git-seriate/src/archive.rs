// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Snapshots written as an archive of a bare repository, with no repository
//! (spec 0466): each object compressed as it is written and kept so, then
//! one pack, without deltas, its objects sorted by id, and its index, in a
//! tar with `HEAD`, `config` and `packed-refs`. The same objects and
//! branches give the same archive, byte for byte, whatever order they were
//! written in.

use std::collections::BTreeMap;
use std::io::Write;

use gix::object::Kind;
use gix::ObjectId;

use crate::Error;

/// An object, compressed.
struct Packed {
    kind: Kind,
    size: usize,
    deflated: Vec<u8>,
}

/// The objects and branches of an archive being built.
#[derive(Default)]
pub struct Archive {
    objects: BTreeMap<ObjectId, Packed>,
    branches: BTreeMap<String, ObjectId>,
}

impl Archive {
    pub fn new() -> Self {
        Self::default()
    }

    /// `data`, an object of `kind`: its id. Written once per id.
    pub fn write(&mut self, kind: Kind, data: &[u8]) -> Result<ObjectId, Error> {
        let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, data).map_err(Error::git)?;
        if let std::collections::btree_map::Entry::Vacant(slot) = self.objects.entry(id) {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(data)?;
            slot.insert(Packed {
                kind,
                size: data.len(),
                deflated: z.finish()?,
            });
        }
        Ok(id)
    }

    /// Where branch `name` points in the archive, if it is there.
    pub fn branch(&self, name: &str) -> Option<ObjectId> {
        self.branches.get(name).copied()
    }

    pub fn set_branch(&mut self, name: &str, id: ObjectId) {
        self.branches.insert(name.to_string(), id);
    }

    /// The archive written to `path`, or to standard output for `-`.
    pub fn write_to_path(&self, path: &std::path::Path) -> Result<(), Error> {
        if path.as_os_str() == "-" {
            self.write_tar(std::io::BufWriter::new(std::io::stdout().lock()))
        } else {
            self.write_tar(std::io::BufWriter::new(std::fs::File::create(path)?))
        }
    }

    /// The archive, as a tar (spec 0466 S2).
    pub fn write_tar(&self, out: impl Write) -> Result<(), Error> {
        let (pack, index, hash) = self.pack()?;
        let mut tar = tar::Builder::new(out);
        let head = self
            .branches
            .keys()
            .next()
            .map_or("refs/heads/main".to_string(), |b| format!("refs/heads/{b}"));
        add_file(&mut tar, "HEAD", format!("ref: {head}\n").as_bytes())?;
        add_file(
            &mut tar,
            "config",
            b"[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = true\n",
        )?;
        for dir in [
            "refs/",
            "refs/heads/",
            "objects/",
            "objects/info/",
            "objects/pack/",
        ] {
            add_dir(&mut tar, dir)?;
        }
        // Sorted, as the header says: git looks refs up by binary search.
        let mut packed_refs = String::from("# pack-refs with: peeled fully-peeled sorted\n");
        for (name, id) in &self.branches {
            packed_refs.push_str(&format!("{id} refs/heads/{name}\n"));
        }
        add_file(&mut tar, "packed-refs", packed_refs.as_bytes())?;
        add_file(&mut tar, &format!("objects/pack/pack-{hash}.pack"), &pack)?;
        add_file(&mut tar, &format!("objects/pack/pack-{hash}.idx"), &index)?;
        tar.into_inner()?.flush()?;
        Ok(())
    }

    /// A version 2 pack of the objects, sorted by id, without deltas; its
    /// version 2 index; its hash.
    fn pack(&self) -> Result<(Vec<u8>, Vec<u8>, ObjectId), Error> {
        let mut pack = Vec::new();
        pack.extend_from_slice(b"PACK");
        pack.extend_from_slice(&2u32.to_be_bytes());
        let count = u32::try_from(self.objects.len())
            .map_err(|_| Error::Refused("more than 2^32 objects in one archive".into()))?;
        pack.extend_from_slice(&count.to_be_bytes());
        let mut entries = Vec::with_capacity(self.objects.len());
        for (id, object) in &self.objects {
            let offset = pack.len() as u64;
            let start = pack.len();
            pack.extend(entry_header(type_id(object.kind), object.size));
            pack.extend_from_slice(&object.deflated);
            let mut crc = flate2::Crc::new();
            crc.update(&pack[start..]);
            entries.push((*id, crc.sum(), offset));
        }
        let hash = sha1(&pack)?;
        pack.extend_from_slice(hash.as_bytes());
        let index = index(&entries, &hash)?;
        Ok((pack, index, hash))
    }
}

fn sha1(bytes: &[u8]) -> Result<ObjectId, Error> {
    let mut h = gix::hash::hasher(gix::hash::Kind::Sha1);
    h.update(bytes);
    h.try_finalize().map_err(Error::git)
}

/// The pack's object type numbers.
fn type_id(kind: Kind) -> u8 {
    match kind {
        Kind::Commit => 1,
        Kind::Tree => 2,
        Kind::Blob => 3,
        Kind::Tag => 4,
    }
}

/// An entry's header: its type and size, 4 bits of the size, then 7 at a
/// time.
fn entry_header(kind: u8, size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut byte = (kind & 0x7) << 4 | (size & 0x0f) as u8;
    let mut rest = size >> 4;
    while rest > 0 {
        out.push(byte | 0x80);
        byte = (rest & 0x7f) as u8;
        rest >>= 7;
    }
    out.push(byte);
    out
}

/// A version 2 index of `entries` (id, CRC-32, offset), sorted by id.
fn index(entries: &[(ObjectId, u32, u64)], pack: &ObjectId) -> Result<Vec<u8>, Error> {
    let mut idx = vec![0xff, b't', b'O', b'c'];
    idx.extend_from_slice(&2u32.to_be_bytes());
    let mut fanout = [0u32; 256];
    for (id, _, _) in entries {
        fanout[id.as_bytes()[0] as usize] += 1;
    }
    let mut total = 0;
    for count in fanout {
        total += count;
        idx.extend_from_slice(&total.to_be_bytes());
    }
    for (id, _, _) in entries {
        idx.extend_from_slice(id.as_bytes());
    }
    for (_, crc, _) in entries {
        idx.extend_from_slice(&crc.to_be_bytes());
    }
    // Offsets past 2^31 in a table of 64-bit ones.
    let mut large = Vec::new();
    for (_, _, offset) in entries {
        let small = match u32::try_from(*offset) {
            Ok(o) if o < 0x8000_0000 => o,
            _ => {
                large.push(*offset);
                0x8000_0000 | (large.len() as u32 - 1)
            }
        };
        idx.extend_from_slice(&small.to_be_bytes());
    }
    for offset in large {
        idx.extend_from_slice(&offset.to_be_bytes());
    }
    idx.extend_from_slice(pack.as_bytes());
    let hash = sha1(&idx)?;
    idx.extend_from_slice(hash.as_bytes());
    Ok(idx)
}

fn header(mode: u32, size: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_size(size);
    header.set_mode(mode);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    header
}

fn add_file<W: Write>(tar: &mut tar::Builder<W>, path: &str, data: &[u8]) -> Result<(), Error> {
    let mut header = header(0o100644, data.len() as u64);
    tar.append_data(&mut header, path, data)?;
    Ok(())
}

fn add_dir<W: Write>(tar: &mut tar::Builder<W>, path: &str) -> Result<(), Error> {
    let mut header = header(0o040755, 0);
    header.set_entry_type(tar::EntryType::Directory);
    header.set_cksum();
    tar.append_data(&mut header, path, std::io::empty())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_headers() {
        // Type 3 (blob), size 5: one byte.
        assert_eq!(entry_header(3, 5), [0x35]);
        // Size 300 = 0b1_0010_1100: 0xc, then 300 >> 4 = 18.
        assert_eq!(entry_header(3, 300), [0x80 | 0x3c, 18]);
    }
}
