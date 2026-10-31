use std::{
    io::{self, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

pub struct PipeClient {
    stream: UnixStream,
}

impl PipeClient {
    pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        Ok(Self { stream })
    }
}

impl Read for PipeClient {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf)
    }
}

impl Write for PipeClient {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

pub struct PipeServer {
    listener: UnixListener,
    path: PathBuf,
}

impl PipeServer {
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path_buf = path.as_ref().to_path_buf();
        if path_buf.exists() {
            let _ = std::fs::remove_file(&path_buf);
        }
        let listener = UnixListener::bind(&path_buf)?;
        Ok(Self {
            listener,
            path: path_buf,
        })
    }

    pub fn accept(&mut self) -> io::Result<PipeClient> {
        let (stream, _) = self.listener.accept()?;
        Ok(PipeClient { stream })
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub enum SingleInstance {
    Forwarded,
    Primary(PipeServer),
    Unavailable(io::Error),
}

pub fn acquire_single_instance(
    path: &Path,
    command: &[u8],
    attempts: u32,
    delay: Duration,
) -> SingleInstance {
    let mut attempt = 1;
    loop {
        if let Ok(mut stream) = PipeClient::connect(path) {
            match stream.write_all(command).and_then(|_| stream.flush()) {
                Ok(()) => return SingleInstance::Forwarded,
                Err(error) => {
                    eprintln!("Failed to forward command to existing Stremio instance: {error}")
                }
            }
        }
        match PipeServer::bind(path) {
            Ok(server) => return SingleInstance::Primary(server),
            Err(_error) if attempt < attempts => {}
            Err(error) => return SingleInstance::Unavailable(error),
        }
        attempt += 1;
        thread::sleep(delay);
    }
}
