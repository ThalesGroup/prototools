// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The blob, wrapped once at load and kept for the session (spec 0216
//! S28, S29).
//!
//! Everything protolens shows is a byte range of *this* buffer. Spec 0114
//! §1.1 makes the document as a whole the sole field of a virtual
//! encompassing message, so the bytes actually decoded are a real
//! tag+length prefix followed by the file's own.
//!
//! Nothing forces the prefix to be written first. Reserve headroom ahead
//! of the payload, fill the payload, then write the prefix into the tail
//! of that headroom and start the buffer wherever it landed. That spares
//! a second allocation and copy, and the width of the prefix never has to
//! be predicted — which matters, because on the text branch the payload's
//! length is not known until it has been encoded.
//!
//! That leaves two producers with one shape: a **binary** file's bytes go
//! into the payload region directly (mapped, or read), and a
//! **`#@ prototext`** file's are encoded into it. Which one runs is
//! decided by [`Blob::load`] from the first 13 bytes.

use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::ops::Deref;
use std::path::Path;

use prototext_core::helpers::{write_tag, write_varint, WT_LEN};
use prototext_core::is_prototext_text;
use prototext_core::serialize::encode_text::encode_text_to_binary_into;

/// Bytes reserved ahead of the payload for the wrapper prefix.
///
/// Generous on purpose: the wrapper is always field 1, so its tag is one
/// byte and its length varint at most five, but nothing here depends on
/// that and a headroom that cannot be too small is one fewer thing to
/// prove. The unused leading bytes are never part of any view.
const HEADROOM: usize = 11;

/// The wrapper's field number (spec 0114 §1.1).
const WRAPPER_FIELD: u32 = 1;

/// Below this, mapping is not worth its setup: the pages it would save
/// are few, and the per-type instance blobs a reader opens by the hundred
/// are all a fraction of it. Nothing depends on the exact value.
const MMAP_MIN_LEN: u64 = 1024 * 1024;

/// The wrapped blob: the wrapper prefix and the payload, contiguous.
///
/// Derefs to the *wrapped* bytes, since that is what every span
/// coordinate in the document is relative to. The payload alone — the
/// file as it is on disk — is [`Blob::payload`].
pub struct Blob {
    store: Store,
    /// Where the wrapper prefix starts in `store`.
    start: usize,
    /// Width of the wrapper prefix. `start + prefix` is the payload.
    prefix: usize,
    /// One past the payload's last byte, in `store`.
    end: usize,
}

impl Blob {
    /// Read or map `path`, then wrap it.
    ///
    /// `assume_binary` skips the peek and takes the file for wire bytes
    /// whatever it starts with. `eager_read` declines mapping — see
    /// [`Blob::load`]'s module documentation for why that is a separate
    /// axis from the format.
    pub fn load(path: &Path, assume_binary: bool, eager_read: bool) -> io::Result<Blob> {
        let mut file = File::open(path)?;

        // The peek costs one short read and decides which producer runs,
        // which has to happen before either buffer exists.
        let is_text = !assume_binary && {
            let mut magic = [0u8; 13];
            let n = read_up_to(&mut file, &mut magic)?;
            file.seek(SeekFrom::Start(0))?;
            is_prototext_text(&magic[..n])
        };

        if is_text {
            let mut text = Vec::new();
            file.read_to_end(&mut text)?;
            // No reservation here beyond the headroom: sizing the payload
            // needs to know how much the encode transiently wastes on
            // placeholders, which is prototext-core's business, and
            // `encode_text_to_binary_into` reserves its own bound on top
            // of whatever `buf` already holds.
            let mut buf = vec![0u8; HEADROOM];
            encode_text_to_binary_into(&text, &mut buf);
            drop(text);
            // The reservation is an upper bound, and a wire blob is several
            // times smaller than its textual form, so most of it is slack —
            // and it is not merely reserved: the encode touches everything
            // up to its own high-water mark, because a MESSAGE placeholder
            // occupies the buffer until compaction removes it. Those pages
            // are resident, and this buffer lives as long as the session,
            // so hand them back now. On a buffer this size the shrink is a
            // `mremap` of the tail rather than a copy.
            buf.shrink_to_fit();
            return Ok(Blob::from_headroom(buf));
        }

        let len = file.metadata()?.len();
        // Both fallbacks are silent by design: a fifo and a small file are
        // ordinary ways to run protolens, not mistakes to report.
        let mappable = !eager_read && len >= MMAP_MIN_LEN && file.metadata()?.is_file();
        if mappable {
            #[cfg(unix)]
            if let Ok(blob) = map::map_and_wrap(&file, len as usize) {
                return Ok(blob);
            }
        }

        let mut buf = Vec::with_capacity(HEADROOM + len as usize);
        buf.resize(HEADROOM, 0);
        file.read_to_end(&mut buf)?;
        Ok(Blob::from_headroom(buf))
    }

    /// Wrap bytes already in memory, for a caller that never had a file.
    #[cfg(test)]
    pub fn wrap(payload: &[u8]) -> Blob {
        let mut buf = Vec::with_capacity(HEADROOM + payload.len());
        buf.resize(HEADROOM, 0);
        buf.extend_from_slice(payload);
        Blob::from_headroom(buf)
    }

    /// Take `bytes` as the whole blob, with no wrapper at all.
    ///
    /// For tests, which mostly want a handful of wire bytes and a
    /// `wrapper_offset` of zero rather than a faithful document.
    #[cfg(test)]
    pub fn unwrapped(bytes: Vec<u8>) -> Blob {
        let end = bytes.len();
        Blob {
            store: Store::Owned(bytes),
            start: 0,
            prefix: 0,
            end,
        }
    }

    /// `buf` is `HEADROOM` bytes of scratch followed by the payload.
    fn from_headroom(mut buf: Vec<u8>) -> Blob {
        let payload_len = buf.len() - HEADROOM;
        let mut prefix = Vec::with_capacity(HEADROOM);
        write_tag(WRAPPER_FIELD, WT_LEN, &mut prefix);
        write_varint(payload_len as u64, &mut prefix);
        let start = HEADROOM - prefix.len();
        buf[start..HEADROOM].copy_from_slice(&prefix);
        let end = buf.len();
        Blob {
            store: Store::Owned(buf),
            start,
            prefix: prefix.len(),
            end,
        }
    }

    /// The wrapper's own tag+length prefix, in bytes.
    ///
    /// Subtract it from any coordinate in the wrapped blob to recover the
    /// file's own numbering.
    pub fn wrapper_offset(&self) -> usize {
        self.prefix
    }

    /// The file's bytes, without the wrapper.
    pub fn payload(&self) -> &[u8] {
        &self.store.bytes()[self.start + self.prefix..self.end]
    }

    /// Whether the payload is the file's own pages rather than a copy.
    pub fn is_mapped(&self) -> bool {
        #[cfg(unix)]
        {
            matches!(self.store, Store::Mapped(_))
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

/// What [`Blob::load`] would have produced from a file holding `bytes`.
///
/// For the tests that call `decode` with a byte literal rather than a
/// file, of which there are dozens.
#[cfg(test)]
pub fn wrapped(bytes: &[u8]) -> std::sync::Arc<Blob> {
    std::sync::Arc::new(Blob::wrap(bytes))
}

impl Deref for Blob {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.store.bytes()[self.start..self.end]
    }
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Blob")
            .field("len", &self.len())
            .field("wrapper_offset", &self.prefix)
            .field("mapped", &self.is_mapped())
            .finish()
    }
}

enum Store {
    Owned(Vec<u8>),
    #[cfg(unix)]
    Mapped(map::Mapping),
}

impl Store {
    fn bytes(&self) -> &[u8] {
        match self {
            Store::Owned(v) => v,
            #[cfg(unix)]
            Store::Mapped(m) => m.as_slice(),
        }
    }
}

/// Read into `buf` until it is full or the file ends, returning how much.
///
/// `read` is free to return less than asked for without being at EOF, so
/// a single call is not enough to decide the magic is absent.
fn read_up_to(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

#[cfg(unix)]
mod map {
    //! The mapped producer.
    //!
    //! The headroom survives mapping because the prefix can sit flush
    //! against the file's first byte: reserve one page plus the rounded-up
    //! file length anonymously, then map the file read-only *over the
    //! tail* with `MAP_FIXED`. The leading page stays writable and the
    //! prefix goes at its end. One contiguous region, nothing written
    //! back, and the whole thing is released by a single `munmap`.
    //!
    //! What this buys is resident memory, not startup time — the walk
    //! touches every byte anyway, so the gain is that file-backed clean
    //! pages are evictable where an anonymous buffer must be swapped.
    //! What it costs is that a file truncated or unmounted underneath a
    //! session becomes a `SIGBUS` rather than a load-time `errno`, which
    //! is what `--eager-read` exists to avoid.

    use std::fs::File;
    use std::io;
    use std::os::unix::io::AsRawFd as _;

    use super::{Blob, Store, WRAPPER_FIELD};
    use prototext_core::helpers::{write_tag, write_varint, WT_LEN};

    pub(super) struct Mapping {
        base: *mut libc::c_void,
        total: usize,
    }

    // The mapping is written only here, during construction, and is
    // read-only afterwards; the file pages are mapped `PROT_READ`. So a
    // `Blob` is as shareable as a plain `Vec`, which matters because the
    // heat worker holds one across threads.
    unsafe impl Send for Mapping {}
    unsafe impl Sync for Mapping {}

    impl Mapping {
        pub(super) fn as_slice(&self) -> &[u8] {
            // SAFETY: `base` is a live mapping of `total` bytes for as
            // long as `self` is, and nothing hands out a `&mut` to it.
            unsafe { std::slice::from_raw_parts(self.base as *const u8, self.total) }
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: `base`/`total` are exactly what `mmap` returned, and
            // the `MAP_FIXED` overlay is part of the same region, so one
            // `munmap` releases both.
            unsafe {
                libc::munmap(self.base, self.total);
            }
        }
    }

    pub(super) fn map_and_wrap(file: &File, len: usize) -> io::Result<Blob> {
        let page = page_size();
        let total = page + len.div_ceil(page) * page;

        // SAFETY: a fresh anonymous reservation at an address of the
        // kernel's choosing; no arguments are derived from user data
        // beyond `total`.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                total,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        // Owned from here on, so every path below unmaps on the way out.
        let mapping = Mapping { base, total };

        // SAFETY: `base + page` is inside the reservation we just took,
        // and `len` rounded up to a page still is. `MAP_FIXED` replaces
        // that part of it atomically.
        let placed = unsafe {
            libc::mmap(
                (base as *mut u8).add(page) as *mut libc::c_void,
                len,
                libc::PROT_READ,
                libc::MAP_PRIVATE | libc::MAP_FIXED,
                file.as_raw_fd(),
                0,
            )
        };
        if placed == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }

        let mut prefix = Vec::with_capacity(super::HEADROOM);
        write_tag(WRAPPER_FIELD, WT_LEN, &mut prefix);
        write_varint(len as u64, &mut prefix);
        let start = page - prefix.len();
        // SAFETY: the leading page is the anonymous, writable part of the
        // reservation, and `prefix` ends flush against the file's first
        // byte.
        unsafe {
            std::ptr::copy_nonoverlapping(
                prefix.as_ptr(),
                (base as *mut u8).add(start),
                prefix.len(),
            );
        }

        Ok(Blob {
            store: Store::Mapped(mapping),
            start,
            prefix: prefix.len(),
            end: page + len,
        })
    }

    fn page_size() -> usize {
        // SAFETY: a plain query with no arguments.
        let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        // `sysconf` returns -1 only for an unrecognized name.
        if n > 0 {
            n as usize
        } else {
            4096
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use prototext_core::helpers::{parse_varint, parse_wiretag};
    use prototext_core::serialize::encode_text::encode_text_to_binary;

    use super::*;

    /// A file under this test's own name, removed when the guard drops.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, bytes: &[u8]) -> TempFile {
            let path =
                std::env::temp_dir().join(format!("protolens-blob-{name}-{}", std::process::id()));
            std::fs::write(&path, bytes).expect("temp file");
            TempFile(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A `#@ prototext` document whose encoding is not its own bytes.
    const TEXT: &[u8] = b"#@ prototext: protoc\nn: 7  #@ int32 = 3\n";

    /// Spec 0216 test item 12. The prefix is written *backwards* into the
    /// headroom, so the one thing that could go wrong is landing it at
    /// the wrong offset when its own width changes. Each length here is
    /// either side of a varint width boundary.
    #[test]
    fn the_wrapper_prefix_lands_flush_at_every_varint_width() {
        for len in [0usize, 1, 127, 128, 16_383, 16_384] {
            let payload: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let blob = Blob::wrap(&payload);

            assert_eq!(blob.payload(), &payload[..], "payload lost at len {len}");
            assert_eq!(
                blob.len(),
                blob.wrapper_offset() + len,
                "wrapped length disagrees with the prefix width at len {len}",
            );

            // The prefix must be a real tag+length, not just the right
            // number of bytes: this is what the decoder will read.
            let tag = parse_wiretag(&blob, 0);
            assert_eq!(tag.wfield, Some(WRAPPER_FIELD as u64), "at len {len}");
            assert_eq!(tag.wtype, Some(WT_LEN), "at len {len}");
            let varint = parse_varint(&blob, tag.next_pos);
            assert_eq!(varint.varint, Some(len as u64), "at len {len}");
            assert_eq!(
                varint.next_pos,
                blob.wrapper_offset(),
                "the prefix does not end where the payload begins, at len {len}",
            );
        }
    }

    /// Spec 0216 test item 13. The peek decides which producer runs, and
    /// `--assume-binary` overrules it.
    #[test]
    fn the_magic_selects_the_text_producer_and_assume_binary_overrules_it() {
        let file = TempFile::new("text", TEXT);

        let decoded = Blob::load(&file.0, false, false).expect("load");
        assert_eq!(
            decoded.payload(),
            &encode_text_to_binary(TEXT)[..],
            "the text was not encoded",
        );

        let verbatim = Blob::load(&file.0, true, false).expect("load");
        assert_eq!(
            verbatim.payload(),
            TEXT,
            "--assume-binary still went down the text branch",
        );
    }

    /// Spec 0216 test item 13. Mapping is an optimization with two silent
    /// fallbacks, so what matters is that declining it changes nothing
    /// about the bytes.
    #[test]
    fn declining_to_map_produces_the_same_blob() {
        // Over the threshold, so the size test alone would not decline.
        let payload = [0x08u8, 0x05].repeat(MMAP_MIN_LEN as usize);
        let file = TempFile::new("large", &payload);

        let mapped = Blob::load(&file.0, true, false).expect("load");
        let read = Blob::load(&file.0, true, true).expect("load");

        // Asserted, even though a mapping failure is deliberately silent
        // in production: without this the test would still pass with the
        // mapped producer never running at all, and then it would prove
        // nothing about the thing it is named for.
        #[cfg(unix)]
        assert!(mapped.is_mapped(), "an over-threshold file was not mapped");
        assert!(!read.is_mapped(), "--eager-read mapped anyway");
        assert_eq!(read.payload(), &payload[..]);
        assert_eq!(&mapped[..], &read[..], "mapping changed the wrapped bytes");
    }

    /// Spec 0216 test item 13. A blob below the threshold is not worth a
    /// mapping's setup, and saying so must not be an error.
    #[test]
    fn a_small_blob_is_read_without_complaint() {
        let file = TempFile::new("small", &[0x08u8, 0x05]);
        let blob = Blob::load(&file.0, true, false).expect("load");

        assert!(!blob.is_mapped(), "a two-byte blob was mapped");
        assert_eq!(blob.payload(), &[0x08, 0x05]);
    }
}
