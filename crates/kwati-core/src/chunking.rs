/// Splits `[offset, offset + total_len)` into contiguous non-overlapping sub-ranges of at most
/// `chunk_size` bytes each, in ascending order.
pub fn split_range(offset: u64, total_len: u32, chunk_size: u32) -> Vec<(u64, u32)> {
    if total_len == 0 || chunk_size == 0 {
        return Vec::new();
    }

    let mut chunks = Vec::with_capacity((total_len as usize).div_ceil(chunk_size as usize));
    let mut remaining = total_len;
    let mut cursor = offset;
    while remaining > 0 {
        let this_chunk = remaining.min(chunk_size);
        chunks.push((cursor, this_chunk));
        cursor += u64::from(this_chunk);
        remaining -= this_chunk;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_zero_chunk_returns_no_chunks() {
        assert_eq!(split_range(1000, 0, 256), Vec::new());
        assert_eq!(split_range(0, 1000, 0), Vec::new());
    }

    #[test]
    fn splits_non_uniform_and_large_ranges_contiguously() {
        assert_eq!(split_range(500, 100, 256), vec![(500, 100)]);
        assert_eq!(
            split_range(0, 700, 256),
            vec![(0, 256), (256, 256), (512, 188)]
        );

        let chunks = split_range(12_345, 10_000_003, 262_144);
        let mut cursor = 12_345u64;
        for (start, len) in chunks {
            assert_eq!(start, cursor);
            assert!(len > 0 && len <= 262_144);
            cursor += u64::from(len);
        }
        assert_eq!(cursor, 12_345 + 10_000_003);
    }
}
