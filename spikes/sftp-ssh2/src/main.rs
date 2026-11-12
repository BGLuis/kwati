//! Spike #36 (b): ssh2 (libssh2) called from a plain blocking thread.
//! Usage: spike-sftp-ssh2 <port> <user> <client_key> <known_hosts> <remote_dir> <remote_file> <remote_out>
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;

use ssh2::{CheckResult, KnownHostFileKind, Session, Sftp};

fn connect(
    host: &str,
    port: u16,
    user: &str,
    key: &Path,
    known_hosts: &Path,
) -> Result<(Session, Sftp), Box<dyn std::error::Error>> {
    let tcp = TcpStream::connect((host, port))?;
    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake()?;

    let mut kh = session.known_hosts()?;
    kh.read_file(known_hosts, KnownHostFileKind::OpenSSH)?;
    let (key_bytes, _key_type) = session.host_key().ok_or("server sent no host key")?;
    match kh.check_port(host, port, key_bytes) {
        CheckResult::Match => {}
        other => return Err(format!("host key check failed: {other:?}").into()),
    }

    session.userauth_pubkey_file(user, None, key, None)?;
    if !session.authenticated() {
        return Err("authentication rejected".into());
    }
    let sftp = session.sftp()?;
    Ok((session, sftp))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let [_, port, user, key, known_hosts, dir, file, out] = &a[..] else {
        return Err("usage: <port> <user> <client_key> <known_hosts> <remote_dir> <remote_file> <remote_out>".into());
    };
    let (_session, sftp) =
        connect("127.0.0.1", port.parse()?, user, Path::new(key), Path::new(known_hosts))?;

    for (path, stat) in sftp.readdir(Path::new(dir))? {
        println!(
            "list: {} size={} dir={}",
            path.file_name().unwrap().to_string_lossy(),
            stat.size.unwrap_or(0),
            stat.is_dir()
        );
    }
    let mut text = String::new();
    // ssh2::File is `Read + Write + Send`, i.e. already the shape of Backend::open_read/open_write.
    let mut r: Box<dyn Read + Send> = Box::new(sftp.open(Path::new(file))?);
    r.read_to_string(&mut text)?;
    println!("read: {:?}", text.trim());

    let mut w: Box<dyn Write + Send> = Box::new(sftp.create(Path::new(out))?);
    w.write_all(b"written-by-ssh2\n")?;
    w.flush()?;
    drop(w);
    println!("write: ok");
    Ok(())
}
