//! Spike for issue #37: connect to a share, list it and read a file with `pavao` (libsmbclient).
//! Usage: pavao-spike <host> <port> <share> <user> <password> <file> [nt1]
//! With `nt1` the client is allowed to negotiate SMB1; by default it is left to libsmbclient's own default.

use pavao::{SmbClient, SmbCredentials, SmbDialect, SmbOpenOptions, SmbOptions};
use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let (host, port, share, user, pass, file) = (&a[1], &a[2], &a[3], &a[4], &a[5], &a[6]);

    let mut options = SmbOptions::default().one_share_per_server(true);
    if a.get(7).map(String::as_str) == Some("nt1") {
        options = options.min_protocol(SmbDialect::Nt1).max_protocol(SmbDialect::Smb311);
    }

    let client = SmbClient::new(
        SmbCredentials::default()
            .server(format!("smb://{host}:{port}"))
            .share(format!("/{share}"))
            .username(user)
            .password(pass)
            .workgroup("WORKGROUP"),
        options,
    )?;
    println!("connected to smb://{host}:{port}/{share}");

    for e in client.list_dir("/")? {
        println!("  {} ({:?})", e.name(), e.get_type());
    }

    let mut f = client.open_with(format!("/{file}"), SmbOpenOptions::default().read(true))?;
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    println!("read {} bytes: {s:?}", s.len());
    Ok(())
}
