use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

pub fn socket_path(override_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return Ok(path);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR is unset; pass --socket PATH")?;
    let directory = PathBuf::from(runtime).join("ttser");
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    Ok(directory.join("control.sock"))
}

pub struct Server {
    pub listener: UnixListener,
    path: PathBuf,
    _lock: File,
}

impl Server {
    pub fn bind(path: PathBuf) -> Result<Self> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        lock.try_lock_exclusive()
            .context("A ttser daemon already owns this socket")?;
        if path.exists() {
            ensure!(
                std::os::unix::fs::FileTypeExt::is_socket(
                    &fs::symlink_metadata(&path)?.file_type()
                ),
                "Refusing to remove non-socket {}",
                path.display()
            );
            fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path).context("Binding control socket")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            path,
            _lock: lock,
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        // Keep the lock inode: removing it would let two waiters lock different files.
    }
}

pub fn read_command(stream: &UnixStream) -> Result<String> {
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    let mut line = String::new();
    BufReader::new(stream.take(65)).read_line(&mut line)?;
    ensure!(
        line.ends_with('\n') && line.len() <= 64,
        "Invalid control request"
    );
    Ok(line.trim().to_owned())
}

pub fn reply(mut stream: &UnixStream, response: &str) -> Result<()> {
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    writeln!(stream, "{}", response.replace(['\n', '\r'], " "))?;
    Ok(())
}

pub fn request(path: &Path, command: &str) -> Result<String> {
    let mut stream = UnixStream::connect(path).with_context(|| {
        format!(
            "Cannot reach {}. Start `ttser serve` first.",
            path.display()
        )
    })?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    writeln!(stream, "{command}")?;
    let mut reply = String::new();
    BufReader::new(stream.take(4096)).read_line(&mut reply)?;
    if reply.is_empty() {
        bail!("Daemon disconnected without a response");
    }
    Ok(reply.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_is_exclusive_private_and_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");
        let server = Server::bind(path.clone()).unwrap();
        assert!(Server::bind(path.clone()).is_err());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(server);
        assert!(!path.exists());
        let _server = Server::bind(path).unwrap();
    }

    #[test]
    fn rejects_overlong_or_incomplete_commands() {
        let (mut client, server) = UnixStream::pair().unwrap();
        client.write_all(&[b'x'; 65]).unwrap();
        assert!(read_command(&server).is_err());
        let (mut client, server) = UnixStream::pair().unwrap();
        client.write_all(b"start\n").unwrap();
        assert_eq!(read_command(&server).unwrap(), "start");
    }

    #[test]
    fn preserves_non_socket_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("important");
        fs::write(&path, "keep me").unwrap();
        assert!(Server::bind(path.clone()).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "keep me");
    }
}
