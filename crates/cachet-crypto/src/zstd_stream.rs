//! Incremental zstd decoding over ruzstd (CLAUDE.md §5). The upload
//! verification pipeline streams a stored NAR through this decoder while a
//! hasher counts: a bomb is bounded by output accounting, never by buffer
//! size. Errors are typed and total: truncated frames, corrupt frames, and
//! over-limit output all answer in [`ZstdError`], and no input shape can
//! panic the decoder.
//!
//! The stream's contract with ruzstd: the frame decoder is invoked only
//! when the input pipe holds a watermark of [`ZSTD_BLOCK_WIRE_MAX`] bytes.
//! A zstd block body can never exceed 128 KiB on the wire, so every block
//! the decoder starts is guaranteed complete in its input, and a
//! mid-block truncation can only ever happen at a real end of input.

use std::cell::RefCell;
use std::io::Read;
use std::rc::Rc;

/// Bytes buffered ahead of the decoder: a zstd block body tops out at
/// 128 KiB on the wire, plus header and trailer slack. Any block started
/// with this much pending input is guaranteed completable, which is what
/// makes error mapping total: after the watermark, a failure can only be
/// corruption.
const ZSTD_BLOCK_WIRE_MAX: usize = 132_096;

/// Bytes buffered before the decoder's header probe: the largest frame
/// header prefix a real producer emits.
const HEADER_PROBE_MAX: usize = 64;

/// Consumed input is dropped once the drained prefix reaches this. The
/// pipe must never hold the whole object: the worker isolate caps linear
/// memory at 128 MiB and honest NARs compress to several hundred MiB
/// (wrangler 4.94 is 167 MiB on the wire), so an append-only buffer dies
/// with alloc::handle_alloc_error mid-verify. Bounded input, bounded
/// decode window, bounded feed slices: verification memory stays flat at
/// any object size.
const PIPE_COMPACT_AT: usize = 4 * 1024 * 1024;

/// Why a decode failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZstdError {
    /// The frame bytes are corrupt.
    CorruptFrame,
    /// The frame produced more bytes than the caller's limit allows: a
    /// bomb guard, enforced stream-side rather than by buffering.
    LimitExceeded,
    /// The stream ended mid-frame: the declared NarSize was not
    /// reproducible from these bytes.
    Truncated,
}

impl core::fmt::Display for ZstdError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::CorruptFrame => "corrupt zstd frame",
            Self::LimitExceeded => "decompressed size over the declared limit",
            Self::Truncated => "zstd stream ended mid-frame",
        })
    }
}

impl std::error::Error for ZstdError {}

/// The shared input pipe: the decoder reads from it, the producer appends
/// to it. An `io::Read` over a cursor into a shared vector, so appending
/// compressed chunks while a streaming decoder holds the pipe needs no
/// unsafe and no threads (workerd isolates are single-threaded).
#[derive(Clone, Default)]
struct Pipe {
    inner: Rc<RefCell<std::io::Cursor<Vec<u8>>>>,
}

impl Pipe {
    /// Append compressed bytes; the cursor position is preserved so the
    /// reader resumes exactly where it left off.
    fn append(&self, bytes: &[u8]) {
        self.inner.borrow_mut().get_mut().extend_from_slice(bytes);
    }

    /// The unread position, for header-probe rewinds.
    fn position(&self) -> u64 {
        self.inner.borrow().position()
    }

    /// Rewind to a saved position: a failed probe must leave the reader
    /// exactly where it started, so the next attempt replays identical
    /// bytes.
    fn set_position(&self, position: u64) {
        self.inner.borrow_mut().set_position(position);
    }

    /// Bytes the reader has not yet consumed.
    fn pending(&self) -> usize {
        let cursor = self.inner.borrow();
        cursor.get_ref().len() - usize::try_from(cursor.position()).expect("cursor ≤ len")
    }

    /// Drop the consumed prefix. Legal only while a decoder is running:
    /// the header-probe rewind (`set_position` in `push`) is the sole
    /// consumer of absolute positions, it resolves inside the `push` call
    /// that opened the probe, and a live decoder holds its own scratch, so
    /// nothing outside this type can ever observe the shift.
    fn compact(&self) {
        let mut cursor = self.inner.borrow_mut();
        let position = usize::try_from(cursor.position()).expect("cursor ≤ len");
        if position >= PIPE_COMPACT_AT {
            cursor.get_mut().drain(..position);
            cursor.set_position(0);
        }
    }
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.borrow_mut().read(buf)
    }
}

/// The incremental decoder. `push` feeds compressed chunks and returns
/// what they decoded to; `finish` closes the stream and validates that
/// the frame completed.
pub struct ZstdStream {
    pipe: Pipe,
    // why: FrameDecoder::init reads the frame header eagerly, and a header
    // can span pushes, so construction defers until the pipe has enough to
    // satisfy it; on failure the cursor rewinds and the next push retries
    // over identical bytes plus new ones.
    decoder: Option<Box<ruzstd::FrameDecoder>>,
    limit: u64,
    decompressed: u64,
    closed: bool,
}

impl ZstdStream {
    /// Start a decode with an output cap. The cap should be the
    /// narinfo-declared NarSize: an honest NAR just fits, and a bomb
    /// cannot write past it.
    pub fn new(limit: u64) -> Self {
        Self {
            pipe: Pipe::default(),
            decoder: None,
            limit,
            decompressed: 0,
            closed: false,
        }
    }

    /// Feed compressed bytes, returning the decoded output they produced.
    /// Output may lag input (frames decode in blocks); `finish` drains the
    /// remainder.
    ///
    /// # Errors
    ///
    /// [`ZstdError`] on corrupt frames, over-limit output, or (after
    /// [`ZstdStream::finish`]) on a stream that ends mid-frame.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<u8>, ZstdError> {
        // Drain before growing: once the decoder exists, no probe rewind
        // is outstanding, so the consumed prefix is dead weight.
        if self.decoder.is_some() {
            self.pipe.compact();
        }
        self.pipe.append(bytes);
        let mut produced = Vec::new();

        if self.decoder.is_none() {
            let can_probe = self.pipe.pending() >= HEADER_PROBE_MAX || self.closed;
            if can_probe {
                let position = self.pipe.position();
                let mut decoder = Box::new(ruzstd::FrameDecoder::new());
                match decoder.init(&mut self.pipe) {
                    Ok(()) => self.decoder = Some(decoder),
                    Err(ruzstd::frame_decoder::FrameDecoderError::FailedToInitialize(_)) => {
                        self.pipe.set_position(position);
                        return Err(ZstdError::CorruptFrame);
                    }
                    Err(_) => {
                        self.pipe.set_position(position);
                        return if self.closed {
                            Err(ZstdError::Truncated)
                        } else {
                            Ok(produced)
                        };
                    }
                }
            } else {
                return Ok(produced);
            }
        }

        // Decode one block per pass, each started only behind the watermark
        // (or at close), then collect. UptoBlocks(1) is the only safe
        // strategy here: BlockDecodingStrategy::All keeps decoding until
        // the frame finishes, and a pipelined source runs dry mid-frame on
        // any partial feed. ruzstd answers that as a read error, which the
        // map below would mislabel corruption. A block started behind the
        // watermark is guaranteed complete on the wire, so the next pass
        // either has a full block pending or waits for the producer.
        while !self
            .decoder
            .as_ref()
            .expect("decoder built above")
            .is_finished()
            && (self.pipe.pending() >= ZSTD_BLOCK_WIRE_MAX || self.closed)
        {
            let result = self
                .decoder
                .as_mut()
                .expect("decoder built above")
                .decode_blocks(&mut self.pipe, ruzstd::BlockDecodingStrategy::UptoBlocks(1));
            match result {
                Ok(_) => {
                    self.collect(&mut produced)?;
                    self.pipe.compact();
                }
                Err(error) => {
                    return Err(self.map_frame_error(&error));
                }
            }
        }
        Ok(produced)
    }

    /// Drain the decoder's collectable buffer with limit accounting.
    fn collect(&mut self, produced: &mut Vec<u8>) -> Result<(), ZstdError> {
        loop {
            let collected = self.decoder.as_mut().expect("decoder built").collect();
            let Some(bytes) = collected else {
                return Ok(());
            };
            if bytes.is_empty() {
                return Ok(());
            }
            self.decompressed = self
                .decompressed
                .checked_add(u64::try_from(bytes.len()).expect("len fits"))
                .ok_or(ZstdError::LimitExceeded)?;
            if self.decompressed > self.limit {
                return Err(ZstdError::LimitExceeded);
            }
            produced.extend_from_slice(&bytes);
        }
    }

    /// Frame errors mean corruption after the watermark (a block is always
    /// complete there); under it, on close, they mean truncation when the
    /// pipe drained empty mid-frame.
    fn map_frame_error(&self, error: &ruzstd::frame_decoder::FrameDecoderError) -> ZstdError {
        let _ = error;
        if self.closed && self.pipe.pending() == 0 {
            ZstdError::Truncated
        } else {
            ZstdError::CorruptFrame
        }
    }

    /// Signal the end of input and drain, returning the decoded tail.
    /// The frame must complete inside the already-fed bytes.
    ///
    /// # Errors
    ///
    /// [`ZstdError::Truncated`] when the frame never completed.
    pub fn finish(&mut self) -> Result<Vec<u8>, ZstdError> {
        self.closed = true;
        let tail = self.push(&[])?;
        let decoder = self.decoder.as_mut().expect("decoder built in push");
        if !decoder.is_finished() {
            return Err(ZstdError::Truncated);
        }
        Ok(tail)
    }

    /// Total decompressed bytes seen so far.
    pub const fn decompressed_bytes(&self) -> u64 {
        self.decompressed
    }
}

/// One decoding pass over an in-memory frame: used by tests, the workerd
/// lane's harness, and the CLI.
///
/// # Errors
///
/// [`ZstdError`] on corrupt or truncated frames and on output over
/// `limit` bytes.
pub fn decode_all(bytes: &[u8], limit: u64) -> Result<Vec<u8>, ZstdError> {
    let mut stream = ZstdStream::new(limit);
    let mut out = stream.push(bytes)?;
    out.extend(stream.finish()?);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAR_ZST: &[u8] = include_bytes!(
        "../../../fixtures/nix-signed/11lx23nn3dpc8mqp0ncnm6wqcxs6pfw32bp8n9c1fkafyzjvn16y.nar.zst"
    );
    const NAR_SHA256: &str = "fa15060475f784debf7dc8331ceca9e76ae633559df1f0f9860654afec2d95c5";

    #[test]
    fn decodes_the_fixture_frame() {
        let plain = decode_all(NAR_ZST, 1_000_000).expect("the fixture decodes");
        assert_eq!(hex::encode(crate::sha256::sha256(&plain)), NAR_SHA256);
    }

    #[test]
    fn awkward_chunking_decodes_identically() {
        for chunk_size in [1usize, 7, 61, 4096] {
            let mut stream = ZstdStream::new(1_000_000);
            let mut out = Vec::new();
            for chunk in NAR_ZST.chunks(chunk_size) {
                out.extend(stream.push(chunk).expect("chunks decode"));
            }
            out.extend(stream.finish().expect("the frame completes"));
            assert_eq!(hex::encode(crate::sha256::sha256(&out)), NAR_SHA256);
        }
    }

    #[test]
    fn a_truncated_frame_is_typed_not_panic() {
        let truncated = &NAR_ZST[..NAR_ZST.len() / 2];
        assert_eq!(
            decode_all(truncated, 1_000_000).unwrap_err(),
            ZstdError::Truncated
        );
    }

    #[test]
    fn corruption_fails_cleanly() {
        let mut corrupted = NAR_ZST.to_vec();
        for byte in &mut corrupted[4..40] {
            *byte ^= 0xff;
        }
        assert!(matches!(
            decode_all(&corrupted, 1_000_000),
            Err(ZstdError::CorruptFrame | ZstdError::Truncated)
        ));
    }

    #[test]
    fn the_limit_is_enforced() {
        assert_eq!(
            decode_all(NAR_ZST, 10).unwrap_err(),
            ZstdError::LimitExceeded
        );
    }

    /// A syntactically honest multi-block frame, built byte by byte:
    /// single segment off, checksum flag on, raw blocks at the 128 KiB
    /// wire cap, any four bytes as the checksum (ruzstd reads it without
    /// validating). The composed NAR incident showed the decoder dying the
    /// first time `push` saw more than one block's worth of a multi-block
    /// frame, and the fixture is too small to reach the watermark: these
    /// frames are big enough to hold that shape in test.
    fn multi_block_frame(blocks: u32) -> (Vec<u8>, Vec<u8>) {
        let block_bytes = 131_072usize;
        let count = usize::try_from(blocks).expect("fits");
        let mut plain = Vec::with_capacity(block_bytes * count);
        for block in 0..blocks {
            plain.extend((0..block_bytes).map(|i| {
                u8::try_from((block * 251 + u32::try_from(i).expect("fits")) % 233)
                    .expect("mod fits")
            }));
        }
        let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0xc4, 0x58];
        frame.extend_from_slice(&u64::try_from(plain.len()).expect("fits").to_le_bytes());
        for (block, body) in plain.chunks(block_bytes).enumerate() {
            let last = u32::from(block == count - 1);
            let header = (u32::try_from(body.len()).expect("block fits") << 3) | last;
            frame.extend_from_slice(&header.to_le_bytes()[..3]);
            frame.extend_from_slice(body);
        }
        frame.extend_from_slice(&[0xef, 0xbe, 0xad, 0xde]);
        (frame, plain)
    }

    #[test]
    fn a_multi_block_frame_decodes_over_every_chunking() {
        let (frame, plain) = multi_block_frame(3);
        // The incident shape: feeds large enough to hold one or more
        // whole blocks with a partial block trailing. Before the fix,
        // the first such feed died with CorruptFrame at decode_blocks
        // running dry mid-frame.
        for chunk in [
            1usize,
            7,
            61,
            4096,
            65_536,
            131_072,
            ZSTD_BLOCK_WIRE_MAX - 1,
            ZSTD_BLOCK_WIRE_MAX,
            ZSTD_BLOCK_WIRE_MAX + 1,
            327_690,
            1_048_576,
            frame.len(),
        ] {
            let mut stream = ZstdStream::new(u64::try_from(plain.len()).expect("fits"));
            let mut out = Vec::new();
            for piece in frame.chunks(chunk) {
                out.extend(stream.push(piece).expect("a partial feed decodes"));
            }
            out.extend(stream.finish().expect("the frame completes"));
            assert_eq!(out, plain, "chunking {chunk} decodes identically");
            assert_eq!(
                stream.decompressed_bytes(),
                u64::try_from(plain.len()).expect("fits")
            );
        }
    }

    #[test]
    fn a_multi_block_frame_may_lag_input_by_a_block() {
        let (frame, plain) = multi_block_frame(3);
        let mut stream = ZstdStream::new(u64::try_from(plain.len()).expect("fits"));
        // One block short of the watermark: nothing provisional decodes,
        // and no error pretends the partial tail is corruption.
        let first: &[u8] = &frame[..131_075];
        let produced = stream.push(first).expect("a block-sized first feed");
        assert_eq!(
            produced.len(),
            0,
            "under the watermark, nothing decodes yet"
        );
        let mut out = stream
            .push(&frame[131_075..])
            .expect("the remainder decodes");
        out.extend(stream.finish().expect("the frame completes"));
        assert_eq!(out, plain);
    }

    #[test]
    fn the_pipe_drains_instead_of_holding_the_object() {
        // 96 blocks ≈ 12.6 MiB on the wire: three compactions' worth. An
        // append-only pipe ends such a run holding every compressed byte,
        // which is what killed verification for 100+ MiB objects inside
        // the isolate's 128 MiB. With drainage the held bytes stay within
        // the threshold, the watermark, and one feed.
        let (frame, plain) = multi_block_frame(96);
        let mut stream = ZstdStream::new(u64::try_from(plain.len()).expect("fits"));
        let mut out = Vec::new();
        for piece in frame.chunks(1_048_576) {
            out.extend(stream.push(piece).expect("feeds decode"));
            let held = stream.pipe.inner.borrow().get_ref().len();
            assert!(
                held <= PIPE_COMPACT_AT + ZSTD_BLOCK_WIRE_MAX + 1_048_576,
                "the pipe holds {held} bytes mid-stream"
            );
        }
        out.extend(stream.finish().expect("the frame completes"));
        assert_eq!(out, plain);
    }
}
