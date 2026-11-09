//! Reading `.deb` archives entirely in memory with pure-Rust decoders.

use std::io::{Cursor, Read, Write};

use anyhow::{Context, Result, anyhow, bail};

use crate::control::Control;

#[derive(Debug)]
pub struct DebArchive {
    pub control: Control,
    /// The decompressed `data.tar` stream.
    pub data_tar: Vec<u8>,
}

/// Parse an `ar`-format Debian package. Nothing from it is executed: the
/// `control.tar` is only searched for the `control` file, and maintainer
/// scripts (`preinst`, `postinst`, …) are ignored by design.
pub fn open_deb(bytes: &[u8], limit: u64) -> Result<DebArchive> {
    let mut archive = ar::Archive::new(Cursor::new(bytes));
    let mut seen_magic = false;
    let mut control = None;
    let mut data = None;

    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.context("malformed ar archive")?;
        let name = String::from_utf8_lossy(entry.header().identifier()).trim_end_matches('/').to_owned();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        match name.as_str() {
            "debian-binary" => {
                if !buf.starts_with(b"2.") {
                    bail!("unsupported deb format version {:?}", String::from_utf8_lossy(&buf).trim());
                }
                seen_magic = true;
            }
            n if n.starts_with("control.tar") => {
                let tar = decompress(n, &buf, limit)?;
                control = Some(Control::parse(&read_control_file(&tar)?));
            }
            n if n.starts_with("data.tar") => data = Some(decompress(n, &buf, limit)?),
            _ => {}
        }
    }
    if !seen_magic {
        bail!("not a Debian package (missing debian-binary member)");
    }
    Ok(DebArchive {
        control: control.ok_or_else(|| anyhow!("deb has no control.tar"))?,
        data_tar: data.ok_or_else(|| anyhow!("deb has no data.tar"))?,
    })
}

fn read_control_file(tar_bytes: &[u8]) -> Result<String> {
    let mut tar = tar::Archive::new(tar_bytes);
    for entry in tar.entries()? {
        let entry = entry?;
        let path = entry.path()?.into_owned();
        if path.as_os_str() == "./control" || path.as_os_str() == "control" {
            let mut s = String::new();
            entry.take(1 << 20).read_to_string(&mut s)?;
            return Ok(s);
        }
    }
    bail!("control.tar has no control file")
}

/// Decompress an archive member chosen by its file extension, refusing to
/// produce more than `limit` bytes (decompression-bomb guard).
pub fn decompress(member: &str, data: &[u8], limit: u64) -> Result<Vec<u8>> {
    if member.ends_with(".xz") {
        decompress_xz(data, limit)
    } else if member.ends_with(".gz") {
        read_limited(flate2::read::GzDecoder::new(data), limit)
    } else if member.ends_with(".zst") {
        let decoder = ruzstd::decoding::StreamingDecoder::new(data).map_err(|e| anyhow!("zstd: {e}"))?;
        read_limited(decoder, limit)
    } else if member.ends_with(".tar") {
        read_limited(data, limit)
    } else {
        bail!("unsupported compression for {member}")
    }
}

pub fn decompress_xz(data: &[u8], limit: u64) -> Result<Vec<u8>> {
    let mut out = LimitedVec { buf: Vec::new(), limit };
    lzma_rs::xz_decompress(&mut &data[..], &mut out).map_err(|e| anyhow!("xz: {e:?}"))?;
    Ok(out.buf)
}

fn read_limited(r: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    r.take(limit + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > limit {
        bail!("decompressed size exceeds limit of {limit} bytes");
    }
    Ok(buf)
}

struct LimitedVec {
    buf: Vec<u8>,
    limit: u64,
}

impl Write for LimitedVec {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if (self.buf.len() + data.len()) as u64 > self.limit {
            return Err(std::io::Error::other("decompressed size limit exceeded"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a minimal but valid .deb: debian-binary, control.tar.gz, data.tar.gz.
    pub fn build_deb(control: &str, files: &[(&str, &[u8], u32)]) -> Vec<u8> {
        fn gz_tar(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
            let mut b = tar::Builder::new(Vec::new());
            for (path, data, mode) in entries {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(*mode);
                h.set_entry_type(tar::EntryType::Regular);
                b.append_data(&mut h, path, *data).unwrap();
            }
            let tar = b.into_inner().unwrap();
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            gz.write_all(&tar).unwrap();
            gz.finish().unwrap()
        }
        let control_tgz = gz_tar(&[("./control", control.as_bytes(), 0o644)]);
        let data_tgz = gz_tar(files);
        let mut out = Vec::new();
        {
            let mut a = ar::Builder::new(&mut out);
            for (name, data) in
                [("debian-binary", b"2.0\n".to_vec()), ("control.tar.gz", control_tgz), ("data.tar.gz", data_tgz)]
            {
                let h = ar::Header::new(name.as_bytes().to_vec(), data.len() as u64);
                a.append(&h, &data[..]).unwrap();
            }
        }
        out
    }

    #[test]
    fn opens_synthetic_deb() {
        let deb =
            build_deb("Package: hello\nVersion: 1.0\nArchitecture: arm64\n", &[("./usr/bin/hello", b"hi", 0o755)]);
        let d = open_deb(&deb, 1 << 20).unwrap();
        assert_eq!(d.control.package(), Some("hello"));
        assert!(!d.data_tar.is_empty());
    }

    #[test]
    fn rejects_non_deb() {
        assert!(open_deb(b"!<arch>\n", 1 << 20).is_err());
    }

    #[test]
    fn decompression_limit_enforced() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(&vec![0u8; 1 << 20]).unwrap();
        let bomb = gz.finish().unwrap();
        assert!(decompress("data.tar.gz", &bomb, 1024).is_err());
    }
}
