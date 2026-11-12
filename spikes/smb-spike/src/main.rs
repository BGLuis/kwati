//! Spike for issue #37: connect to a share, list it and read a file with the `smb` crate.
//! Usage: smb-spike <host> <port> <share> <user> <password> <file>

use smb::{Client, ClientConfig, ConnectionConfig, Directory, FileAccessMask, FileCreateArgs, UncPath};
use smb::ReadAt;
use smb_fscc::{DirAccessMask, FileIdBothDirectoryInformation};
use std::str::FromStr;

#[cfg(feature = "tokio")]
use futures_util::StreamExt;

#[cfg(feature = "tokio")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run().await
}

#[cfg(not(feature = "tokio"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}

#[maybe_async::maybe_async]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let (host, port, share, user, pass, file) = (&a[1], a[2].parse::<u16>()?, &a[3], &a[4], &a[5], &a[6]);

    let mut config = ClientConfig::default();
    config.connection = ConnectionConfig { port: Some(port), ..Default::default() };
    let client = Client::new(config);

    let root = UncPath::from_str(&format!(r"\\{host}\{share}"))?;
    client.share_connect(&root, user, pass.clone()).await?;
    println!("connected to {root}");

    let dir = client
        .create_file(
            &root,
            &FileCreateArgs::make_open_existing(
                DirAccessMask::new().with_list_directory(true).with_synchronize(true).into(),
            ),
        )
        .await?
        .unwrap_dir();
    let dir = std::sync::Arc::new(dir);
    list(&dir).await?;

    let f = client
        .create_file(
            &root.clone().with_path(file),
            &FileCreateArgs::make_open_existing(FileAccessMask::new().with_generic_read(true)),
        )
        .await?
        .unwrap_file();
    let mut buf = vec![0u8; 4096];
    let n = f.read_at(&mut buf, 0).await?;
    println!("read {n} bytes: {:?}", String::from_utf8_lossy(&buf[..n]));
    f.close().await?;
    Ok(())
}

#[cfg(feature = "tokio")]
async fn list(dir: &std::sync::Arc<Directory>) -> Result<(), Box<dyn std::error::Error>> {
    let mut it = Directory::query::<FileIdBothDirectoryInformation>(dir, "*").await?;
    while let Some(e) = it.next().await {
        let e = e?;
        println!("  {} ({} bytes)", e.file_name, e.end_of_file);
    }
    Ok(())
}

#[cfg(not(feature = "tokio"))]
fn list(dir: &std::sync::Arc<Directory>) -> Result<(), Box<dyn std::error::Error>> {
    for e in Directory::query::<FileIdBothDirectoryInformation>(dir, "*")? {
        let e = e?;
        println!("  {} ({} bytes)", e.file_name, e.end_of_file);
    }
    Ok(())
}
