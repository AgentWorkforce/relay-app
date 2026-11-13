//! A loopback HTTP server that stands in for Control's upload-credentials
//! route, the workspace's S3 bucket and the receiver at once: every request
//! goes to one responder, which answers by method and path, and is recorded
//! for the test to inspect after the client has returned.
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

/// One request as it arrived.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// The path and query as sent.
    pub target: String,
    /// Header names lowercased, in the order sent.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(sent, _)| sent == name)
            .map(|(_, value)| value.as_str())
    }
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }
    /// The decoded value of query parameter `name`.
    pub fn query(&self, name: &str) -> Option<String> {
        let (_, query) = self.target.split_once('?')?;
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// An answer: a status, extra headers and a body.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}
impl Reply {
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.to_string(),
        }
    }
    pub fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![],
            body: body.into(),
        }
    }
    pub fn with(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

pub struct FakeS3 {
    pub base: String,
    stop: Arc<AtomicBool>,
    handle: thread::JoinHandle<Vec<Request>>,
}
impl FakeS3 {
    /// Serve until joined. `respond` gets the server's own base URL with
    /// each request.
    pub fn serve(mut respond: impl FnMut(&str, &Request) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let own = base.clone();
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            loop {
                // Read the flag before accepting: every connection the client
                // made is queued before it returns.
                let done = stopped.load(Ordering::SeqCst);
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if done {
                            return requests;
                        }
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                let request = read(&stream);
                let reply = respond(&own, &request);
                requests.push(request);
                let mut stream = stream;
                let mut head = format!(
                    "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reply.body.len()
                );
                for (name, value) in &reply.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                let _ = write!(stream, "{head}\r\n{}", reply.body);
            }
        });
        Self { base, stop, handle }
    }
    /// Every request served, once the client has returned.
    pub fn join(self) -> Vec<Request> {
        self.stop.store(true, Ordering::SeqCst);
        self.handle.join().unwrap()
    }
}

fn read(stream: &std::net::TcpStream) -> Request {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        let (name, value) = (name.to_ascii_lowercase(), value.trim().to_owned());
        if name == "content-length" {
            length = value.parse().unwrap();
        }
        headers.push((name, value));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    Request {
        method,
        target,
        headers,
        body,
    }
}
