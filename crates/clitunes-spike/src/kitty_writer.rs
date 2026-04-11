use base64::Engine;
use std::io::Write;

const ESC: &str = "\x1b";
const APC_START: &str = "\x1b_G";
const APC_END: &str = "\x1b\\";
const CHUNK_BYTES: usize = 4096;

pub struct KittyWriter<W: Write> {
    out: W,
    image_id: u32,
}

impl<W: Write> KittyWriter<W> {
    pub fn new(out: W) -> Self {
        Self { out, image_id: 1 }
    }

    /// Move cursor to home so the image redraws in the same place each frame.
    pub fn cursor_home(&mut self) -> std::io::Result<()> {
        write!(self.out, "{}[H", ESC)
    }

    pub fn clear_screen(&mut self) -> std::io::Result<()> {
        write!(self.out, "{}[2J{}[H", ESC, ESC)
    }

    /// Transmit one frame of f=32 RGBA pixel data with the same image id so
    /// the terminal updates in place. Returns bytes written to the output sink.
    pub fn write_frame(&mut self, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<usize> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(rgba);
        let bytes = b64.as_bytes();
        let total = bytes.len();
        let chunks: Vec<&[u8]> = bytes.chunks(CHUNK_BYTES).collect();
        let n = chunks.len();
        let mut written = 0;
        for (i, chunk) in chunks.iter().enumerate() {
            let is_last = i + 1 == n;
            let m = if is_last { 0 } else { 1 };
            if i == 0 {
                // First chunk carries all metadata.
                let header = format!(
                    "a=T,f=32,s={w},v={h},i={id},q=2,m={m}",
                    w = width,
                    h = height,
                    id = self.image_id,
                    m = m
                );
                self.out.write_all(APC_START.as_bytes())?;
                self.out.write_all(header.as_bytes())?;
                self.out.write_all(b";")?;
                self.out.write_all(chunk)?;
                self.out.write_all(APC_END.as_bytes())?;
                written += APC_START.len() + header.len() + 1 + chunk.len() + APC_END.len();
            } else {
                let header = format!("m={m}", m = m);
                self.out.write_all(APC_START.as_bytes())?;
                self.out.write_all(header.as_bytes())?;
                self.out.write_all(b";")?;
                self.out.write_all(chunk)?;
                self.out.write_all(APC_END.as_bytes())?;
                written += APC_START.len() + header.len() + 1 + chunk.len() + APC_END.len();
            }
        }
        let _ = total;
        Ok(written)
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}
