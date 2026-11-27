use crate::server::{handle, header, library, reply};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use frametape::{
    TapeReader,
    transfer::{
        CHUNK, HOST, MAX_MEMBER, Member, digest_bytes, digest_file, id_for, member_name, valid_id,
        valid_member,
    },
};
use serde_json::json;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tiny_http::{Method, Request, Response, Server};

struct Portal {
    incoming: PathBuf,
    credentials: Option<Credentials>,
    origins: Vec<String>,
}

struct Credentials {
    password: String,
    token: String,
}

fn header_value<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

fn verifier(name: &str) -> io::Result<String> {
    let value = std::env::var(name).map_err(io::Error::other)?;
    (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(value)
        .ok_or_else(|| io::Error::other(format!("invalid {name}")))
}

fn equal(a: &str, b: &str) -> bool {
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn serve(path: PathBuf, port: u16) -> io::Result<()> {
    const PASSWORD: &str = "FRAMETAPE_PORTAL_PASSWORD_SHA256";
    const TOKEN: &str = "FRAMETAPE_PORTAL_TOKEN_SHA256";
    let credentials = if std::env::var_os(PASSWORD).is_none() && std::env::var_os(TOKEN).is_none() {
        None
    } else {
        let password = verifier(PASSWORD)?;
        let token = verifier(TOKEN)?;
        if equal(&password, &token) {
            return Err(io::Error::other(
                "portal password and upload token must differ",
            ));
        }
        Some(Credentials { password, token })
    };
    private_dir(&path)?;
    let state = library(&path, true)?;
    let incoming = state.recordings.root.join(".incoming");
    private_dir(&incoming)?;
    let portal = Arc::new(Portal {
        incoming,
        credentials,
        origins: std::env::var("FRAMETAPE_PORTAL_WEB_ORIGINS")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
    });
    let mut thread_handles = Vec::new();
    #[cfg(unix)]
    let _socket = if let Some(path) = frametape::transfer::socket_path() {
        private_dir(path.parent().unwrap())?;
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        lock.try_lock().map_err(io::Error::other)?;
        match fs::remove_file(&path) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        let local = Arc::new(Server::http_unix(&path).map_err(io::Error::other)?);
        thread_handles.extend(workers(local, state.clone(), portal.clone(), true));
        Some((path, lock))
    } else {
        None
    };
    if portal.credentials.is_some() {
        let front = Arc::new(Server::http(("127.0.0.1", port)).map_err(io::Error::other)?);
        println!("Portal reverse-proxy listener {}", front.server_addr());
        thread_handles.extend(workers(front, state, portal, false));
    } else if thread_handles.is_empty() {
        return Err(io::Error::other(
            "portal needs an owner socket or both credential verifiers",
        ));
    } else {
        println!("Portal owner socket ready");
    }
    for worker in thread_handles {
        worker
            .join()
            .map_err(|_| io::Error::other("portal worker failed"))?;
    }
    Ok(())
}

fn workers(
    server: Arc<Server>,
    state: crate::server::Library,
    portal: Arc<Portal>,
    local: bool,
) -> Vec<std::thread::JoinHandle<()>> {
    (0..8)
        .map(|_| {
            let (server, state, portal) = (server.clone(), state.clone(), portal.clone());
            std::thread::spawn(move || {
                for request in server.incoming_requests() {
                    if let Err(error) = route(request, &state, &portal, local) {
                        eprintln!("portal request: {error}");
                    }
                }
            })
        })
        .collect()
}

fn route(
    request: Request,
    state: &crate::server::Library,
    portal: &Portal,
    local: bool,
) -> io::Result<()> {
    let host = if local { "localhost" } else { HOST };
    if header_value(&request, "Host") != Some(host) {
        return reply(request, 403, "text/plain", "invalid host");
    }
    let route = request
        .url()
        .split('?')
        .next()
        .unwrap_or("")
        .trim_start_matches('/')
        .to_owned();
    let ingest = route.starts_with("ingest/");
    let origin = header_value(&request, "Origin").map(str::to_owned);
    let cross = origin
        .as_deref()
        .filter(|value| ingest && portal.origins.iter().any(|entry| entry == value));
    if request.method() == &Method::Options && cross.is_some() {
        return ingest_response(request, 204, "", cross);
    }
    if origin
        .as_deref()
        .is_some_and(|value| value != "https://tape.api.haus")
        && cross.is_none()
    {
        return reply(request, 403, "text/plain", "invalid origin");
    }
    if !local {
        let Some(credentials) = &portal.credentials else {
            return reply(request, 503, "text/plain", "portal sign-in unavailable");
        };
        let auth = header_value(&request, "Authorization").unwrap_or("");
        let bearer = ingest
            && auth
                .strip_prefix("Bearer ")
                .is_some_and(|value| equal(&digest_bytes(value.as_bytes()), &credentials.token));
        let authenticated = auth
            .strip_prefix("Basic ")
            .and_then(|value| STANDARD.decode(value).ok())
            .is_some_and(|value| {
                value
                    .strip_prefix(b"owner:")
                    .is_some_and(|password| equal(&digest_bytes(password), &credentials.password))
            });
        if !authenticated && !bearer {
            if ingest {
                return ingest_response(request, 401, "invalid portal credential", cross);
            }
            return request.respond(
                Response::from_string("Frametape owner login")
                    .with_status_code(401)
                    .with_header(header(
                        "WWW-Authenticate",
                        "Basic realm=\"Frametape\", charset=\"UTF-8\"",
                    ))
                    .with_header(header("Cache-Control", "no-store")),
            );
        }
        if !bearer
            && matches!(request.method(), &Method::Post | &Method::Put)
            && origin.as_deref() != Some("https://tape.api.haus")
        {
            return reply(request, 403, "text/plain", "invalid origin");
        }
    }
    if ingest {
        return portal.ingest(request, &route, &state.recordings.root, cross);
    }
    if route == "uploading" {
        let rows = fs::read_dir(&portal.incoming)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let id = entry.file_name().into_string().ok()?;
                let recent = entry
                    .path()
                    .join(".active")
                    .metadata()
                    .ok()?
                    .modified()
                    .ok()?
                    .elapsed()
                    .ok()?
                    .as_secs()
                    < 45;
                (valid_id(&id) && recent).then_some(json!({"id":id}))
            })
            .collect::<Vec<_>>();
        let revision = fs::metadata(&state.recordings.root)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos()
            .to_string();
        return reply(
            request,
            200,
            "application/json",
            json!({"active":rows,"revision":revision}).to_string(),
        );
    }
    if route.is_empty() && request.method() == &Method::Get {
        let html = include_str!("../../../app/catalog.html").replacen(
            "<html lang=\"en\">",
            "<html lang=\"en\" data-portal=\"true\">",
            1,
        );
        return reply(request, 200, "text/html", html);
    }
    handle(request, &route, state)
}

fn ingest_response(
    request: Request,
    status: u16,
    message: &str,
    cross: Option<&str>,
) -> io::Result<()> {
    let mut response = Response::from_string(message)
        .with_status_code(status)
        .with_header(header("Cache-Control", "no-store"));
    if let Some(origin) = cross {
        response = response
            .with_header(header("Access-Control-Allow-Origin", origin))
            .with_header(header("Access-Control-Allow-Methods", "PUT, POST"))
            .with_header(header("Access-Control-Allow-Private-Network", "true"))
            .with_header(header(
                "Access-Control-Allow-Headers",
                "Authorization, Content-Type",
            ))
            .with_header(header("Vary", "Origin"));
    }
    request.respond(response)
}

impl Portal {
    fn ingest(
        &self,
        mut request: Request,
        route: &str,
        root: &Path,
        cross: Option<&str>,
    ) -> io::Result<()> {
        let parts: Vec<_> = route.split('/').collect();
        let Some(id) = parts.get(1).copied().filter(|id| valid_id(id)) else {
            return ingest_response(request, 400, "invalid recording ID", cross);
        };
        let result = (|| {
            let output = root.join(format!("{id}.ftape"));
            if output.is_file() {
                return Ok(());
            }
            let journal = self.incoming.join(id);
            fs::create_dir_all(&journal)?;
            // Commit and updates across connections share the same stable lock.
            let lock = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(journal.join(".upload.lock"))?;
            lock.lock()?;
            if output.is_file() {
                return Ok(());
            }
            match (request.method().clone(), parts.as_slice()) {
                (Method::Put, [_, _, encoded, offset]) => {
                    let name = member_name(encoded)
                        .ok_or_else(|| io::Error::other("invalid member name"))?;
                    let offset: u64 = offset.parse().map_err(io::Error::other)?;
                    let mut content = Vec::new();
                    request
                        .as_reader()
                        .take(CHUNK as u64 + 1)
                        .read_to_end(&mut content)?;
                    if request.body_length() != Some(content.len()) {
                        return Err(io::Error::other("incomplete chunk"));
                    }
                    if content.is_empty()
                        || content.len() > CHUNK
                        || offset > MAX_MEMBER - content.len() as u64
                    {
                        return Err(io::Error::other("invalid chunk size or offset"));
                    }
                    let target = journal.join(name);
                    fs::create_dir_all(target.parent().unwrap())?;
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(target)?;
                    if offset > file.metadata()?.len() {
                        return Err(io::Error::other("chunk would leave a gap"));
                    }
                    file.seek(SeekFrom::Start(offset))?;
                    file.write_all(&content)?;
                    File::create(journal.join(".active"))?;
                }
                (Method::Post, [_, _, "commit"]) => {
                    let mut body = Vec::new();
                    request
                        .as_reader()
                        .take(CHUNK as u64 + 1)
                        .read_to_end(&mut body)?;
                    if body.len() > CHUNK {
                        return Err(io::Error::other("manifest too large"));
                    }
                    commit(&journal, &output, id, serde_json::from_slice(&body)?)?;
                    fs::remove_dir_all(&journal)?;
                }
                _ => return Err(io::Error::other("unknown ingest route")),
            }
            Ok::<_, io::Error>(())
        })();
        match result {
            Ok(()) => ingest_response(request, 204, "", cross),
            Err(error) => {
                // Filesystem paths and host details are available only in the service log.
                eprintln!("portal ingest {id}: {error}");
                ingest_response(request, 422, "recording could not be stored", cross)
            }
        }
    }
}

fn commit(journal: &Path, output: &Path, id: &str, members: Vec<Member>) -> io::Result<()> {
    let mut names = HashSet::new();
    for member in &members {
        if !valid_member(&member.name)
            || !names.insert(member.name.as_str())
            || member.bytes > MAX_MEMBER
        {
            return Err(io::Error::other("invalid recording member"));
        }
        let path = journal.join(&member.name);
        fs::create_dir_all(path.parent().unwrap())?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(member.bytes == 0)
            .truncate(false)
            .open(path)?;
        if file.metadata()?.len() < member.bytes {
            return Err(io::Error::other("recording upload is incomplete"));
        }
        file.set_len(member.bytes)?;
        if !equal(&digest_file(&mut file)?, &member.sha256) {
            return Err(io::Error::other("recording member digest differs"));
        }
        file.sync_all()?;
    }
    let candidate = journal.join(".candidate.ftape");
    // A stale candidate means the service was interrupted before publication.
    let interrupted = candidate.with_extension("ftape.writing");
    if interrupted.exists() {
        fs::remove_file(interrupted)?;
    }
    if candidate.exists() {
        fs::remove_file(&candidate)?;
    }
    let names: Vec<_> = members.into_iter().map(|member| member.name).collect();
    frametape::seal_journal(journal, &names, &candidate)?;
    let result = (|| {
        let mut tape = TapeReader::open(&candidate)?;
        if id_for(&tape.manifest.id) != id {
            return Err(io::Error::other("recording identity mismatch"));
        }
        tape.frames()?;
        tape.annotations()?;
        fs::hard_link(&candidate, output)?;
        #[cfg(unix)]
        File::open(output.parent().unwrap())?.sync_all()?;
        Ok(())
    })();
    fs::remove_file(candidate)?;
    result
}
