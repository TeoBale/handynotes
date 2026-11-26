use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

use url::Url;
use uuid::Uuid;

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(180);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_REQUEST_LINE: u64 = 4096;

/// Receives Convex Auth's one-time code from the system browser. The app keeps
/// the matching verifier in memory and exchanges it directly with Convex.
pub(crate) struct Loopback {
    listener: TcpListener,
    url: Url,
}

impl Loopback {
    pub(crate) fn bind() -> Result<Self, String> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| "Could not start GitHub sign-in on this Mac.")?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "Could not start GitHub sign-in on this Mac.")?;
        let address = listener
            .local_addr()
            .map_err(|_| "Could not find the GitHub sign-in callback address.")?;
        let url = Url::parse(&format!(
            "http://{address}/oauth/callback/{}",
            Uuid::new_v4()
        ))
        .map_err(|_| "Could not create the GitHub sign-in callback URL.")?;
        Ok(Self { listener, url })
    }

    pub(crate) fn url(&self) -> &Url {
        &self.url
    }

    pub(crate) fn wait_for_code(self) -> Result<String, String> {
        let deadline = Instant::now() + SIGN_IN_TIMEOUT;
        while Instant::now() < deadline {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    if let Some(result) = self.handle_callback(&mut stream) {
                        return result;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err("Could not receive the GitHub sign-in callback.".into()),
            }
        }
        Err("GitHub sign-in timed out. Try again.".into())
    }

    fn handle_callback(&self, stream: &mut TcpStream) -> Option<Result<String, String>> {
        stream.set_read_timeout(Some(REQUEST_TIMEOUT)).ok()?;
        stream.set_write_timeout(Some(REQUEST_TIMEOUT)).ok()?;
        let mut request_line = String::new();
        BufReader::new(&mut *stream)
            .take(MAX_REQUEST_LINE)
            .read_line(&mut request_line)
            .ok()?;
        let Some(result) = callback_code(&self.url, &request_line) else {
            respond(stream, "404 Not Found", "Page not found.");
            return None;
        };
        if result.is_ok() {
            respond(
                stream,
                "200 OK",
                "Return to HandyNotes to finish signing in.",
            );
        } else {
            respond(
                stream,
                "400 Bad Request",
                "GitHub sign-in did not complete. Return to HandyNotes to try again.",
            );
        }
        Some(result)
    }
}

fn callback_code(callback: &Url, request_line: &str) -> Option<Result<String, String>> {
    let mut parts = request_line.strip_suffix("\r\n")?.split(' ');
    if parts.next()? != "GET" {
        return None;
    }
    let target = parts.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != callback.path()
        || !matches!(parts.next()?, "HTTP/1.0" | "HTTP/1.1")
        || parts.next().is_some()
    {
        return None;
    }

    let mut codes = url::form_urlencoded::parse(query.as_bytes())
        .filter_map(|(key, value)| (key == "code").then_some(value));
    match codes.next() {
        Some(code) if !code.is_empty() && codes.next().is_none() => Some(Ok(code.into_owned())),
        _ => Some(Err("GitHub sign-in did not complete. Try again.".into())),
    }
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_checks_method_path_and_code() {
        let callback = Url::parse("http://127.0.0.1:54321/oauth/callback/expected").unwrap();

        for request in [
            "GET /favicon.ico HTTP/1.1\r\n",
            "GET /oauth/callback/other?code=wrong HTTP/1.1\r\n",
            "POST /oauth/callback/expected?code=wrong HTTP/1.1\r\n",
        ] {
            assert_eq!(callback_code(&callback, request), None);
        }

        for request in [
            "GET /oauth/callback/expected HTTP/1.1\r\n",
            "GET /oauth/callback/expected?code= HTTP/1.1\r\n",
            "GET /oauth/callback/expected?code=one&code=two HTTP/1.1\r\n",
        ] {
            assert!(callback_code(&callback, request).unwrap().is_err());
        }

        assert_eq!(
            callback_code(
                &callback,
                "GET /oauth/callback/expected?code=a%2Bb HTTP/1.1\r\n"
            ),
            Some(Ok("a+b".to_owned()))
        );
    }
}
