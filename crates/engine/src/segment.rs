//! A segment is a byte range `[pos, end)` still to be downloaded.
//!
//! `pos` moves forward as data is written. `end` can move *backward* when an
//! idle connection steals the second half of this range: that is dynamic
//! segmentation. Both live under one mutex so a write and a split can never
//! disagree about who owns a byte.

use std::fs::File;
use std::io;
use std::sync::Mutex;

pub struct Segment {
    /// Where this segment began (for showing per-connection progress).
    start: u64,
    range: Mutex<(u64, u64)>,
}

impl Segment {
    pub fn new(pos: u64, end: u64) -> Self {
        Self { start: pos, range: Mutex::new((pos, end)) }
    }

    pub fn range(&self) -> (u64, u64) {
        *self.range.lock().unwrap()
    }

    /// `(start, pos, end)`: began at `start`, has written up to `pos`, owns
    /// bytes up to `end`.
    pub fn span(&self) -> (u64, u64, u64) {
        let (pos, end) = self.range();
        (self.start, pos, end)
    }

    pub fn remaining(&self) -> u64 {
        let (pos, end) = self.range();
        end.saturating_sub(pos)
    }

    /// Writes as much of `data` as still belongs to this segment, straight
    /// to its final offset in the file. Returns `false` once the segment is
    /// complete (possibly early, because its tail was stolen), telling the
    /// caller to drop the connection.
    pub fn write(&self, file: &File, data: &[u8]) -> io::Result<bool> {
        let mut r = self.range.lock().unwrap();
        let (pos, end) = *r;
        if pos >= end {
            return Ok(false);
        }
        let n = (data.len() as u64).min(end - pos) as usize;
        write_all_at(file, &data[..n], pos)?;
        r.0 += n as u64;
        Ok(r.0 < r.1)
    }

    /// Gives away the back half of what is left and returns it as a new
    /// segment, or `None` if the remainder is too small to be worth a new
    /// connection.
    pub fn split(&self, min_half: u64) -> Option<Segment> {
        let mut r = self.range.lock().unwrap();
        let (pos, end) = *r;
        let remaining = end.saturating_sub(pos);
        if remaining < min_half * 2 {
            return None;
        }
        let mid = pos + remaining / 2;
        r.1 = mid;
        Some(Segment::new(mid, end))
    }
}

/// Splits `[0, size)` into `n` roughly equal segments.
pub fn initial_segments(size: u64, n: usize) -> Vec<(u64, u64)> {
    let n = (n as u64).clamp(1, size.max(1));
    let chunk = size / n;
    (0..n)
        .map(|i| {
            let start = i * chunk;
            let end = if i == n - 1 { size } else { start + chunk };
            (start, end)
        })
        .collect()
}

#[cfg(windows)]
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_split_covers_file() {
        let segs = initial_segments(1003, 4);
        assert_eq!(segs.first().unwrap().0, 0);
        assert_eq!(segs.last().unwrap().1, 1003);
        for w in segs.windows(2) {
            assert_eq!(w[0].1, w[1].0);
        }
        assert_eq!(initial_segments(3, 8).len(), 3);
    }

    #[test]
    fn split_halves_remaining() {
        let s = Segment::new(100, 300);
        let tail = s.split(10).unwrap();
        assert_eq!(s.range(), (100, 200));
        assert_eq!(tail.range(), (200, 300));
        assert!(Segment::new(0, 15).split(10).is_none());
    }
}
