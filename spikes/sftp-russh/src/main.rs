//! Spike #36 (a): russh + russh-sftp behind a blocking facade.
//! Usage: spike-sftp-russh <port> <user> <client_key> <known_hosts> <remote_dir> <remote_file> <remote_out>
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use russh::client::{self, Handler};
use russh::keys::{self, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::{Handle, Runtime};

struct KnownHostsHandler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
}

impl Handler for KnownHostsHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Ok(false);
        };
        Ok(keys::check_known_hosts_path(&self.host, self.port, key, &self.known_hosts)
            .unwrap_or(false))
    }
}

/// Blocking facade: owns a Tokio runtime whose worker thread drives the SSH
/// connection; callers on any non-Tokio thread (e.g. GPUI's) call plain methods.
struct BlockingSftp {
    rt: Runtime,
    sftp: SftpSession,
}

impl BlockingSftp {
    fn connect(
        host: &str,
        port: u16,
        user: &str,
        key_path: &str,
        known_hosts: PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("kwati-sftp-rt")
            .enable_all()
            .build()?;
        let sftp = rt.block_on(async {
            let key = keys::load_secret_key(key_path, None)?;
            let handler = KnownHostsHandler { host: host.into(), port, known_hosts };
            let mut session =
                client::connect(Arc::new(client::Config::default()), (host, port), handler).await?;
            let auth = session
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), None))
                .await?;
            if !auth.success() {
                return Err("authentication rejected".into());
            }
            let channel = session.channel_open_session().await?;
            channel.request_subsystem(true, "sftp").await?;
            let sftp = SftpSession::new(channel.into_stream()).await?;
            Ok::<_, Box<dyn std::error::Error>>(sftp)
        })?;
        Ok(Self { rt, sftp })
    }

    fn list(&self, dir: &str) -> Result<Vec<(String, u64, bool)>, Box<dyn std::error::Error>> {
        self.rt.block_on(async {
            let mut out = Vec::new();
            for e in self.sftp.read_dir(dir).await? {
                let m = e.metadata();
                out.push((e.file_name(), m.size.unwrap_or(0), e.file_type().is_dir()));
            }
            Ok(out)
        })
    }

    /// Shape of `Backend::open_read -> Box<dyn Read + Send>`.
    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, Box<dyn std::error::Error>> {
        let file = self.rt.block_on(self.sftp.open(path))?;
        Ok(Box::new(BlockingFile { handle: self.rt.handle().clone(), file }))
    }

    /// Shape of `Backend::open_write -> Box<dyn Write + Send>`.
    fn open_write(&self, path: &str) -> Result<Box<dyn Write + Send>, Box<dyn std::error::Error>> {
        let file = self.rt.block_on(self.sftp.create(path))?;
        Ok(Box::new(BlockingFile { handle: self.rt.handle().clone(), file }))
    }
}

struct BlockingFile {
    handle: Handle,
    file: russh_sftp::client::fs::File,
}

impl Read for BlockingFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.handle.block_on(self.file.read(buf))
    }
}

impl Write for BlockingFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.handle.block_on(self.file.write(buf))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.handle.block_on(self.file.flush())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let [_, port, user, key, known_hosts, dir, file, out] = &a[..] else {
        return Err("usage: <port> <user> <client_key> <known_hosts> <remote_dir> <remote_file> <remote_out>".into());
    };
    let c = BlockingSftp::connect("127.0.0.1", port.parse()?, user, key, known_hosts.into())?;

    for (name, size, is_dir) in c.list(dir)? {
        println!("list: {name} size={size} dir={is_dir}");
    }
    let mut text = String::new();
    c.open_read(file)?.read_to_string(&mut text)?;
    println!("read: {:?}", text.trim());

    let mut w = c.open_write(out)?;
    w.write_all(b"written-by-russh\n")?;
    w.flush()?;
    drop(w);
    println!("write: ok");
    Ok(())
}
