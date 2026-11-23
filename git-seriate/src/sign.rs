// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! SSH commit signing (spec 0463): through the agent, or with a key file
//! read once, its passphrase asked for once; `ssh-keygen -Y sign` for what
//! neither can do. The signature is SSHSIG, namespace `git`, SHA-512,
//! armored as `ssh-keygen` armors it: what git writes in `gpgsig`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use ssh_key::{HashAlg, PrivateKey, PublicKey, SshSig};

use crate::Error;

const NAMESPACE: &str = "git";
const HASH: HashAlg = HashAlg::Sha512;

/// `user.signingKey`, as git reads it (S1).
#[derive(Debug, Clone)]
enum KeySpec {
    /// `key::ssh-…`, or a bare `ssh-…`.
    Literal(String),
    /// A path, to a public or a private key.
    Path(PathBuf),
}

/// How the command signs, decided at its first commit (S4).
enum Method {
    Agent(agent::Agent, PublicKey),
    Key(Box<PrivateKey>),
    SshKeygen(PathBuf),
}

pub(crate) struct Signer {
    key: Option<KeySpec>,
    method: RefCell<Option<Method>>,
}

impl Signer {
    pub(crate) fn from_config(repo: &gix::Repository) -> Result<Self, Error> {
        let config = repo.config_snapshot();
        let sign = config.boolean("commit.gpgSign").unwrap_or(false);
        let ssh = config
            .string("gpg.format")
            .is_some_and(|f| f.eq_ignore_ascii_case(b"ssh"));
        let key = if sign && ssh {
            let key = config
                .string("user.signingKey")
                .ok_or_else(|| {
                    Error::Refused("commit.gpgSign is set, but not user.signingKey".into())
                })?
                .to_string();
            Some(key_spec(&key))
        } else {
            None
        };
        Ok(Signer {
            key,
            method: RefCell::new(None),
        })
    }

    pub(crate) fn sign(&self, mut commit: gix::objs::Commit) -> Result<gix::objs::Commit, Error> {
        use gix::objs::WriteTo;
        let Some(key) = &self.key else {
            return Ok(commit);
        };
        let mut payload = Vec::new();
        commit.write_to(&mut payload).map_err(Error::git)?;
        let mut method = self.method.borrow_mut();
        if method.is_none() {
            *method = Some(choose(key)?);
        }
        let armored = match method.as_mut().expect("chosen") {
            Method::Agent(agent, public) => {
                let data = SshSig::signed_data(NAMESPACE, HASH, &payload).map_err(refused)?;
                let signature = agent.sign(public, &data)?;
                let sig = SshSig::new(public.key_data().clone(), NAMESPACE, HASH, signature)
                    .map_err(refused)?;
                armor(&sig)?
            }
            Method::Key(private) => {
                let sig =
                    SshSig::sign(private.as_ref(), NAMESPACE, HASH, &payload).map_err(refused)?;
                armor(&sig)?
            }
            Method::SshKeygen(path) => ssh_keygen(path, &payload)?,
        };
        commit.extra_headers.push(("gpgsig".into(), armored.into()));
        Ok(commit)
    }
}

fn refused(e: impl std::fmt::Display) -> Error {
    Error::Refused(format!("signing: {e}"))
}

fn key_spec(key: &str) -> KeySpec {
    if let Some(literal) = key.strip_prefix("key::") {
        return KeySpec::Literal(literal.to_string());
    }
    if key.starts_with("ssh-") || key.starts_with("ecdsa-") || key.starts_with("sk-") {
        return KeySpec::Literal(key.to_string());
    }
    let path = match key.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(key),
    };
    KeySpec::Path(path)
}

/// S2 to S4: the agent if it holds the key, else the key file, else
/// `ssh-keygen`.
fn choose(key: &KeySpec) -> Result<Method, Error> {
    // The public key, and the private key file, if the path is one.
    let (public, private_path): (Option<PublicKey>, Option<&Path>) = match key {
        KeySpec::Literal(text) => (
            Some(PublicKey::from_openssh(text).map_err(|e| {
                Error::Refused(format!("user.signingKey is not a public key: {e}"))
            })?),
            None,
        ),
        KeySpec::Path(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| Error::Refused(format!("reading {}: {e}", path.display())))?;
            match PublicKey::from_openssh(text.trim()) {
                Ok(public) => (Some(public), None),
                Err(_) => {
                    let private = PrivateKey::from_openssh(&text).map_err(|e| {
                        Error::Refused(format!("{} is not an SSH key: {e}", path.display()))
                    })?;
                    (Some(private.public_key().clone()), Some(path.as_path()))
                }
            }
        }
    };
    if let (Some(public), Some(sock)) = (&public, std::env::var_os("SSH_AUTH_SOCK")) {
        if let Ok(mut agent) = agent::Agent::connect(Path::new(&sock)) {
            if agent.holds(public).unwrap_or(false) {
                return Ok(Method::Agent(agent, public.clone()));
            }
        }
    }
    match (key, private_path) {
        (_, Some(path)) => {
            let text = std::fs::read_to_string(path)?;
            let private = PrivateKey::from_openssh(&text).map_err(refused)?;
            if !in_process(&private) {
                return Ok(Method::SshKeygen(path.to_path_buf()));
            }
            let private = if private.is_encrypted() {
                decrypt(&private, path)?
            } else {
                private
            };
            Ok(Method::Key(Box::new(private)))
        }
        (KeySpec::Path(path), None) => {
            // A public key no agent holds: ssh-keygen says why, as before.
            Ok(Method::SshKeygen(path.clone()))
        }
        (KeySpec::Literal(_), None) => Err(Error::Refused(
            "user.signingKey is a public key, and no agent (SSH_AUTH_SOCK) holds it".into(),
        )),
    }
}

/// S5: Ed25519 and ECDSA in-process. RSA is signed by the agent or by
/// `ssh-keygen`: the `rsa` crate's private-key operations are not
/// constant-time (RUSTSEC-2023-0071), with no fixed release.
fn in_process(key: &PrivateKey) -> bool {
    use ssh_key::{Algorithm, EcdsaCurve};
    matches!(
        key.algorithm(),
        Algorithm::Ed25519
            | Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256 | EcdsaCurve::NistP384
            }
    )
}

/// S3b: the passphrase, once: on the terminal, else from `SSH_ASKPASS`.
fn decrypt(private: &PrivateKey, path: &Path) -> Result<PrivateKey, Error> {
    let prompt = format!("Enter passphrase for {}: ", path.display());
    let require = std::env::var("SSH_ASKPASS_REQUIRE").unwrap_or_default();
    let askpass = std::env::var_os("SSH_ASKPASS").filter(|_| require != "never");
    let tty = require != "force" && std::fs::File::open("/dev/tty").is_ok();
    let refused = || {
        Error::Refused(format!(
            "{} is encrypted and no passphrase could be read; add it to the agent (ssh-add {})",
            path.display(),
            path.display()
        ))
    };
    if tty {
        for _ in 0..3 {
            let Ok(passphrase) = rpassword::prompt_password(&prompt) else {
                break;
            };
            if let Ok(key) = private.decrypt(passphrase.as_bytes()) {
                return Ok(key);
            }
            eprintln!("Bad passphrase.");
        }
        return Err(refused());
    }
    let Some(program) = askpass else {
        return Err(refused());
    };
    let out = std::process::Command::new(&program)
        .arg(&prompt)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|_| refused())?;
    let passphrase = String::from_utf8_lossy(&out.stdout);
    let passphrase = passphrase.strip_suffix('\n').unwrap_or(&passphrase);
    if !out.status.success() {
        return Err(refused());
    }
    private
        .decrypt(passphrase.as_bytes())
        .map_err(|_| Error::Refused(format!("{}: bad passphrase", path.display())))
}

/// `ssh-keygen`'s armor: base64 in lines of 70, then a newline.
fn armor(sig: &SshSig) -> Result<Vec<u8>, Error> {
    use ssh_key::LineEnding;
    let pem = sig.to_pem(LineEnding::LF).map_err(refused)?;
    let mut lines = pem.lines();
    let (Some(begin), mut body) = (lines.next(), String::new()) else {
        return Err(refused("empty armor"));
    };
    let mut end = "";
    for line in lines {
        if line.starts_with("-----END") {
            end = line;
            break;
        }
        body.push_str(line);
    }
    let mut out = String::new();
    out.push_str(begin);
    out.push('\n');
    for chunk in body.as_bytes().chunks(70) {
        out.push_str(std::str::from_utf8(chunk).expect("base64 is ASCII"));
        out.push('\n');
    }
    out.push_str(end);
    out.push('\n');
    Ok(out.into_bytes())
}

/// S4: one `ssh-keygen -Y sign` per commit, as before 0463.
fn ssh_keygen(key: &Path, payload: &[u8]) -> Result<Vec<u8>, Error> {
    use std::io::Write;
    let mut child = std::process::Command::new("ssh-keygen")
        .args(["-Y", "sign", "-n", NAMESPACE, "-f"])
        .arg(key)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| Error::Refused(format!("running ssh-keygen: {e}")))?;
    child.stdin.take().expect("piped").write_all(payload)?;
    let out = child.wait_with_output()?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err(Error::Refused(format!(
            "signing with {}: {}",
            key.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// The agent protocol's two requests git-seriate makes (draft-miller-ssh-
/// agent): the identities, and a signature.
mod agent {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::Path;

    use ssh_key::{Algorithm, PublicKey, Signature};

    use crate::Error;

    const REQUEST_IDENTITIES: u8 = 11;
    const IDENTITIES_ANSWER: u8 = 12;
    const SIGN_REQUEST: u8 = 13;
    const SIGN_RESPONSE: u8 = 14;
    /// `SSH_AGENT_RSA_SHA2_512`: an RSA key signs with SHA-512.
    const RSA_SHA2_512: u32 = 4;

    pub(super) struct Agent(UnixStream);

    impl Agent {
        pub(super) fn connect(sock: &Path) -> std::io::Result<Agent> {
            UnixStream::connect(sock).map(Agent)
        }

        fn request(&mut self, body: &[u8]) -> Result<Vec<u8>, Error> {
            let mut out = Vec::with_capacity(body.len() + 4);
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(body);
            self.0.write_all(&out)?;
            let mut len = [0u8; 4];
            self.0.read_exact(&mut len)?;
            let mut answer = vec![0u8; u32::from_be_bytes(len) as usize];
            self.0.read_exact(&mut answer)?;
            Ok(answer)
        }

        pub(super) fn holds(&mut self, key: &PublicKey) -> Result<bool, Error> {
            let answer = self.request(&[REQUEST_IDENTITIES])?;
            let mut r = Reader(&answer);
            if r.byte()? != IDENTITIES_ANSWER {
                return Ok(false);
            }
            let wanted = key.to_bytes().map_err(super::refused)?;
            for _ in 0..r.u32()? {
                let blob = r.string()?;
                r.string()?; // its comment
                if blob == wanted.as_slice() {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        pub(super) fn sign(&mut self, key: &PublicKey, data: &[u8]) -> Result<Signature, Error> {
            let blob = key.to_bytes().map_err(super::refused)?;
            let flags = if matches!(key.algorithm(), Algorithm::Rsa { .. }) {
                RSA_SHA2_512
            } else {
                0
            };
            let mut body = vec![SIGN_REQUEST];
            put_string(&mut body, &blob);
            put_string(&mut body, data);
            body.extend_from_slice(&flags.to_be_bytes());
            let answer = self.request(&body)?;
            let mut r = Reader(&answer);
            if r.byte()? != SIGN_RESPONSE {
                return Err(Error::Refused("the agent refused to sign".into()));
            }
            let sig = r.string()?;
            let mut s = Reader(sig);
            let algorithm = s.string()?;
            let bytes = s.string()?;
            let algorithm = Algorithm::new(std::str::from_utf8(algorithm).unwrap_or_default())
                .map_err(super::refused)?;
            Signature::new(algorithm, bytes.to_vec()).map_err(super::refused)
        }
    }

    fn put_string(out: &mut Vec<u8>, s: &[u8]) {
        out.extend_from_slice(&(s.len() as u32).to_be_bytes());
        out.extend_from_slice(s);
    }

    struct Reader<'a>(&'a [u8]);

    impl<'a> Reader<'a> {
        fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
            if self.0.len() < n {
                return Err(Error::Refused("a truncated answer from the agent".into()));
            }
            let (head, rest) = self.0.split_at(n);
            self.0 = rest;
            Ok(head)
        }

        fn byte(&mut self) -> Result<u8, Error> {
            Ok(self.take(1)?[0])
        }

        fn u32(&mut self) -> Result<u32, Error> {
            Ok(u32::from_be_bytes(
                self.take(4)?.try_into().expect("4 bytes"),
            ))
        }

        fn string(&mut self) -> Result<&'a [u8], Error> {
            let n = self.u32()? as usize;
            self.take(n)
        }
    }
}
