// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `settle`, end to end (spec 0452 test plan 3–5, 7): repositories in
//! temporary directories, the binary run on them.

use std::path::{Path, PathBuf};
use std::process::Command;

use git_seriate::format::Source;
use git_seriate::store::{self, Item, NewSnapshot};
use seriation::generate::{generate, Params};

const ENV: [(&str, &str); 4] = [
    ("GIT_COMMITTER_NAME", "Tester"),
    ("GIT_COMMITTER_EMAIL", "tester@example.com"),
    ("GIT_AUTHOR_NAME", "Tester"),
    ("GIT_AUTHOR_EMAIL", "tester@example.com"),
];

/// No configuration but the repository's: not the user's (which may sign
/// with their key), not the system's.
fn isolated(command: &mut Command, repo: &Path) {
    let home = repo.parent().unwrap();
    command
        .envs(ENV)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_CONFIG_GLOBAL")
        // Signing (spec 0463): no agent nor askpass of the caller's.
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("SSH_ASKPASS")
        .env_remove("SSH_ASKPASS_REQUIRE");
}

fn git_seriate(repo: &Path, args: &[&str]) -> (bool, String, String) {
    git_seriate_env(repo, args, &[])
}

fn git_seriate_env(repo: &Path, args: &[&str], env: &[(&str, &str)]) -> (bool, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_git-seriate"));
    isolated(&mut command, repo);
    command.envs(env.iter().copied());
    let out = command.arg("-C").arg(repo).args(args).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn git(repo: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    isolated(&mut command, repo);
    let out = command.arg("-C").arg(repo).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn new_repo(root: &Path) -> PathBuf {
    let repo = root.join("repo.git");
    gix::init_bare(&repo).unwrap();
    repo
}

/// A `seriation` corpus's snapshots, added as snapshot branches.
fn add_corpus(repo: &Path, name: &str, filter: impl Fn(usize) -> bool) {
    let args = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../seriation/tests/corpora/{name}.args")),
    )
    .unwrap();
    let corpus = generate(&Params::from_args(args.lines().map(str::to_string)).unwrap());
    let r = gix::open(repo).unwrap();
    for (i, s) in corpus.snapshots.iter().enumerate() {
        if !filter(i) {
            continue;
        }
        store::add(
            &r,
            NewSnapshot {
                source: Source {
                    identity: s.identity.clone(),
                    build_time: s.build_time,
                    order_hint: s.order_hint,
                    group: None,
                    metadata: Vec::new(),
                },
                items: s
                    .files
                    .iter()
                    .map(|(path, text)| Item {
                        path: path.clone(),
                        bytes: text.clone().into_bytes(),
                    })
                    .collect(),
                rejected: Vec::new(),
                record: Vec::new(),
            },
        )
        .unwrap();
    }
}

/// The history, oldest first, as `identity<TAB>tree` lines.
fn history(repo: &Path, branch: &str) -> String {
    let log = git(repo, &["log", "--reverse", "--format=%T %B%x00", branch]);
    log.split('\0')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| {
            let (tree, message) = c.split_once(' ').unwrap();
            let identity = message
                .lines()
                .next()
                .unwrap()
                .trim_start_matches("Snapshot ");
            format!("{identity}\t{tree}\n")
        })
        .collect()
}

fn build_times(repo: &Path) -> Vec<(i64, String)> {
    let r = gix::open(repo).unwrap();
    let mut times: Vec<(i64, String)> = store::list(&r)
        .unwrap()
        .into_iter()
        .map(|l| (l.block.source.build_time, l.branch))
        .collect();
    times.sort();
    times
}

#[test]
fn the_corpora_settle_as_the_existing_implementation_does() {
    for name in ["plain", "noisy", "builds"] {
        let expected = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/histories/{name}.history.tsv")),
        )
        .unwrap();

        // In one go.
        let tmp = tempfile::tempdir().unwrap();
        let repo = new_repo(tmp.path());
        add_corpus(&repo, name, |_| true);
        let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
        assert!(ok, "{name}: {err}");
        let onego = history(&repo, "main");
        assert_eq!(onego, expected, "{name}: one go");
        git(&repo, &["fsck", "--full", "--strict", "--no-dangling"]);

        // The newer half by build time first, then the rest.
        let tmp = tempfile::tempdir().unwrap();
        let repo = new_repo(tmp.path());
        add_corpus(&repo, name, |_| true);
        let times = build_times(&repo);
        let newer: Vec<&str> = times[times.len() / 2..]
            .iter()
            .map(|(_, b)| b.as_str())
            .collect();
        let mut args = vec!["settle", "--into", "main"];
        args.extend(&newer);
        let (ok, _, err) = git_seriate(&repo, &args);
        assert!(ok, "{name}: {err}");
        let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
        assert!(ok, "{name}: {err}");
        assert_eq!(history(&repo, "main"), expected, "{name}: in halves");
    }
}

/// A small history: three snapshots of one growing file.
fn small(repo: &Path) {
    let r = gix::open(repo).unwrap();
    for (i, text) in ["a\n", "a\nb\n", "a\nb\nc\n"].iter().enumerate() {
        store::add(
            &r,
            NewSnapshot {
                source: Source {
                    identity: format!("s{i}"),
                    build_time: 1_700_000_000 + i as i64 * 86_400,
                    order_hint: None,
                    group: None,
                    metadata: Vec::new(),
                },
                items: vec![Item {
                    path: "f.txt".into(),
                    bytes: text.as_bytes().to_vec(),
                }],
                rejected: Vec::new(),
                record: Vec::new(),
            },
        )
        .unwrap();
    }
}

/// A manual commit on `branch` writing `path`.
fn manual_commit(repo: &Path, branch: &str, path: &str, text: &str, subject: &str) {
    let worktree = repo.parent().unwrap().join("wt");
    std::fs::create_dir_all(&worktree).unwrap();
    let index = repo.parent().unwrap().join("index");
    let run = |args: &[&str]| {
        let mut command = Command::new("git");
        isolated(&mut command, repo);
        let out = command
            .arg("--git-dir")
            .arg(repo)
            .arg("--work-tree")
            .arg(&worktree)
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    run(&["read-tree", branch]);
    let file = worktree.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, text).unwrap();
    run(&["add", path]);
    let tree = run(&["write-tree"]);
    let parent = run(&["rev-parse", branch]);
    let commit = run(&["commit-tree", &tree, "-p", &parent, "-m", subject]);
    run(&["update-ref", &format!("refs/heads/{branch}"), &commit]);
}

#[test]
fn nothing_new_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    small(&repo);
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    assert!(out.contains("wrote 3 snapshot(s)"), "{out}");
    let before = git(&repo, &["rev-parse", "main"]);

    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok);
    assert!(err.contains("nothing new to settle"), "{err}");
    // A full re-sort finds the same order, and keeps every commit.
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main", "--no-cutoff"]);
    assert!(ok, "{err}");
    assert!(out.contains("kept 3, wrote 0"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "main"]), before);
    assert!(
        git(&repo, &["branch", "--list", "main-*"]).is_empty(),
        "no backup"
    );
}

#[test]
fn a_manual_commit_follows_its_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    git(&repo, &["config", "seriate.itemPattern", "*.txt"]);
    small(&repo);
    // s0 and s2 first; notes after s2; then s1, built between them.
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main", "s0", "s2"]);
    assert!(ok, "{err}");
    manual_commit(&repo, "main", "NOTES.md", "notes\n", "docs: notes");
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main", "--no-cutoff"]);
    assert!(ok, "{err}");
    assert!(out.contains("replay "), "{out}");
    assert!(out.contains("previous tip saved as main-1"), "{out}");
    let subjects = git(&repo, &["log", "--reverse", "--format=%s", "main"]);
    assert_eq!(
        subjects,
        "Snapshot s0\nSnapshot s1\nSnapshot s2\ndocs: notes\n"
    );
    let message = git(&repo, &["log", "-1", "--format=%B", "main"]);
    assert!(message.contains("Snapshot-Anchor: s2"), "{message}");
    let tip = git(&repo, &["rev-parse", "main"]);

    // Again: the recorded anchor keeps it where it is.
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main", "--no-cutoff"]);
    assert!(ok, "{err}");
    assert!(out.contains("wrote 0"), "{out}");
    assert_eq!(git(&repo, &["rev-parse", "main"]), tip);
    git(&repo, &["fsck", "--full", "--strict", "--no-dangling"]);
}

#[test]
fn a_manual_commit_on_an_item_path_stops_the_walk() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    small(&repo);
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main", "s1", "s2"]);
    assert!(ok, "{err}");
    manual_commit(&repo, "main", "README.md", "hi\n", "docs: readme");
    // No pattern: README.md counts as an item, and the warning says so.
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    assert!(err.contains("settle stopped at"), "{err}");
    assert!(err.contains("no seriate.itemPattern is set"), "{err}");
    let subjects = git(&repo, &["log", "--reverse", "--format=%s", "main"]);
    assert_eq!(
        subjects, "Snapshot s1\nSnapshot s2\ndocs: readme\nSnapshot s0\n",
        "below the stop nothing moves; s0 goes on top"
    );
}

#[test]
fn a_dry_run_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    small(&repo);
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main", "--dry-run"]);
    assert!(ok, "{err}");
    assert!(out.contains("would settle main"), "{out}");
    assert!(git(&repo, &["branch", "--list", "main"]).is_empty());
    let objects = git(&repo, &["count-objects", "-v"]);
    let (ok, _, _) = git_seriate(&repo, &["settle", "--into", "main", "--dry-run"]);
    assert!(ok);
    assert_eq!(
        git(&repo, &["count-objects", "-v"]),
        objects,
        "no object written"
    );
}

#[test]
fn commits_are_signed_when_git_is_configured_to() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    let key = tmp.path().join("key");
    let Ok(made) = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .output()
    else {
        eprintln!("skipped: no ssh-keygen");
        return;
    };
    assert!(made.status.success());
    git(&repo, &["config", "commit.gpgSign", "true"]);
    git(&repo, &["config", "gpg.format", "ssh"]);
    git(&repo, &["config", "user.signingKey", key.to_str().unwrap()]);
    small(&repo);
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main"]);
    assert!(ok, "{err}");
    let commit = git(&repo, &["cat-file", "commit", "main"]);
    assert!(
        commit.contains("gpgsig -----BEGIN SSH SIGNATURE-----"),
        "{commit}"
    );
    // The payload signed is the commit without its signature: git agrees.
    let allowed = tmp.path().join("allowed");
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    std::fs::write(&allowed, format!("tester@example.com {public}")).unwrap();
    git(
        &repo,
        &[
            "config",
            "gpg.ssh.allowedSignersFile",
            allowed.to_str().unwrap(),
        ],
    );
    git(&repo, &["verify-commit", "main"]);
}

// ── Signing in-process (spec 0463) ───────────────────────────────────────────

/// A new key of `kind` (`-t`), with `passphrase`; `None` without
/// ssh-keygen.
fn ssh_key(dir: &Path, name: &str, kind: &str, passphrase: &str) -> Option<PathBuf> {
    let key = dir.join(name);
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", kind, "-N", passphrase, "-C", "", "-f"])
        .arg(&key)
        .output()
        .ok()?;
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    Some(key)
}

fn sign_with(repo: &Path, key: &Path) {
    git(repo, &["config", "commit.gpgSign", "true"]);
    git(repo, &["config", "gpg.format", "ssh"]);
    git(repo, &["config", "user.signingKey", key.to_str().unwrap()]);
}

/// Every commit of `branch` accepted by `git verify-commit` against `pub`.
fn verify_all(repo: &Path, branch: &str, public: &Path) {
    let allowed = repo.parent().unwrap().join("allowed");
    let public = std::fs::read_to_string(public).unwrap();
    std::fs::write(&allowed, format!("tester@example.com {public}")).unwrap();
    git(
        repo,
        &[
            "config",
            "gpg.ssh.allowedSignersFile",
            allowed.to_str().unwrap(),
        ],
    );
    for id in git(repo, &["rev-list", branch]).lines() {
        git(repo, &["verify-commit", id]);
    }
}

/// `commit`'s signature, and the payload it signs: the commit without its
/// `gpgsig` header.
fn signature_and_payload(commit: &str) -> (String, String) {
    let (mut sig, mut payload, mut in_sig) = (String::new(), String::new(), false);
    for line in commit.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("gpgsig ") {
            in_sig = true;
            sig.push_str(rest);
        } else if in_sig && line.starts_with(' ') {
            sig.push_str(&line[1..]);
        } else {
            in_sig = false;
            payload.push_str(line);
        }
    }
    (sig, payload)
}

/// What `ssh-keygen -Y sign` writes for `payload` with `key`, in `env`.
fn ssh_keygen_signs(key: &Path, payload: &str, env: &[(&str, &str)]) -> String {
    use std::io::Write;
    let mut child = Command::new("ssh-keygen")
        .args(["-Y", "sign", "-n", "git", "-f"])
        .arg(key)
        .envs(env.iter().copied())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn signs_like_ssh_keygen_with_a_key_file() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    let Some(key) = ssh_key(tmp.path(), "key", "ed25519", "") else {
        eprintln!("skipped: no ssh-keygen");
        return;
    };
    sign_with(&repo, &key);
    small(&repo);
    // No ssh-keygen on its PATH: signed in-process, or not at all.
    let (ok, _, err) = git_seriate_env(&repo, &["settle", "--into", "main"], &[("PATH", "")]);
    assert!(ok, "{err}");
    for id in git(&repo, &["rev-list", "main"]).lines() {
        let (sig, payload) = signature_and_payload(&git(&repo, &["cat-file", "commit", id]));
        assert_eq!(sig, ssh_keygen_signs(&key, &payload, &[]), "commit {id}");
    }
    verify_all(&repo, "main", &key.with_extension("pub"));
}

/// An `ssh-agent` of the test's, stopped when dropped.
struct TestAgent {
    child: std::process::Child,
    sock: PathBuf,
}

impl TestAgent {
    fn start(dir: &Path) -> Option<TestAgent> {
        let sock = dir.join("agent.sock");
        let child = Command::new("ssh-agent")
            .arg("-D")
            .arg("-a")
            .arg(&sock)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        for _ in 0..100 {
            if sock.exists() {
                return Some(TestAgent { child, sock });
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        None
    }

    fn env(&self) -> [(&str, &str); 1] {
        [("SSH_AUTH_SOCK", self.sock.to_str().unwrap())]
    }

    fn add(&self, key: &Path) {
        self.add_with(key, &[]);
    }

    /// `ssh-add` with `env` (an `SSH_ASKPASS` for an encrypted key).
    fn add_with(&self, key: &Path, env: &[(&str, &str)]) {
        let out = Command::new("ssh-add")
            .arg(key)
            .envs(self.env())
            .envs(env.iter().copied())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

impl Drop for TestAgent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn signs_through_the_agent() {
    for kind in ["rsa", "ecdsa", "ed25519"] {
        let tmp = tempfile::tempdir().unwrap();
        let repo = new_repo(tmp.path());
        let Some(key) = ssh_key(tmp.path(), "key", kind, "") else {
            eprintln!("skipped: no ssh-keygen");
            return;
        };
        let Some(agent) = TestAgent::start(tmp.path()) else {
            eprintln!("skipped: no ssh-agent");
            return;
        };
        agent.add(&key);
        // The private key gone: only the agent can sign.
        std::fs::remove_file(&key).unwrap();
        let public = key.with_extension("pub");
        sign_with(&repo, &public);
        small(&repo);
        let [sock] = agent.env();
        let env = [sock, ("PATH", "")];
        let (ok, _, err) = git_seriate_env(&repo, &["settle", "--into", "main"], &env);
        assert!(ok, "{kind}: {err}");
        verify_all(&repo, "main", &public);
        // RSA (PKCS #1 v1.5) and Ed25519 are deterministic: ssh-keygen,
        // through the same agent, writes the same.
        if kind != "ecdsa" {
            for id in git(&repo, &["rev-list", "main"]).lines() {
                let (sig, payload) =
                    signature_and_payload(&git(&repo, &["cat-file", "commit", id]));
                assert_eq!(
                    sig,
                    ssh_keygen_signs(&public, &payload, &agent.env()),
                    "{kind}"
                );
            }
        }
    }
}

/// An executable script at `path`: `body` after `#!/bin/sh`.
fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Spec 0472 S1, 1: an unencrypted key file is signed from before any agent
/// is asked. The agent here is a socket that counts its connections and
/// answers none.
#[test]
fn an_unencrypted_key_file_is_used_before_the_agent() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    let Some(key) = ssh_key(tmp.path(), "key", "ed25519", "") else {
        eprintln!("skipped: no ssh-keygen");
        return;
    };
    let sock = tmp.path().join("agent.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            count.fetch_add(1, Ordering::SeqCst);
            drop(connection);
        }
    });
    sign_with(&repo, &key);
    small(&repo);
    let env = [("SSH_AUTH_SOCK", sock.to_str().unwrap()), ("PATH", "")];
    let (ok, _, err) = git_seriate_env(&repo, &["settle", "--into", "main"], &env);
    assert!(ok, "{err}");
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the agent was asked");
    verify_all(&repo, "main", &key.with_extension("pub"));
}

/// Spec 0472 S1, 2: an encrypted key file the agent holds is signed through
/// the agent, no passphrase asked; an RSA key file, which isn't signed
/// in-process, the same.
#[test]
fn an_encrypted_or_rsa_key_file_goes_to_the_agent() {
    for (kind, passphrase) in [("ed25519", "secret words"), ("rsa", "")] {
        let tmp = tempfile::tempdir().unwrap();
        let repo = new_repo(tmp.path());
        let Some(key) = ssh_key(tmp.path(), "key", kind, passphrase) else {
            eprintln!("skipped: no ssh-keygen");
            return;
        };
        let Some(agent) = TestAgent::start(tmp.path()) else {
            eprintln!("skipped: no ssh-agent");
            return;
        };
        let give = tmp.path().join("give");
        script(&give, &format!("echo '{passphrase}'"));
        agent.add_with(
            &key,
            &[
                ("SSH_ASKPASS", give.to_str().unwrap()),
                ("SSH_ASKPASS_REQUIRE", "force"),
            ],
        );
        // An askpass that says it was run, and fails.
        let asked = tmp.path().join("asked");
        let refuse = tmp.path().join("refuse");
        script(&refuse, &format!("touch '{}'; exit 1", asked.display()));
        sign_with(&repo, &key);
        small(&repo);
        let [sock] = agent.env();
        let env = [
            sock,
            ("SSH_ASKPASS", refuse.to_str().unwrap()),
            ("SSH_ASKPASS_REQUIRE", "force"),
            // No ssh-keygen: the agent signs, or nothing does.
            ("PATH", ""),
        ];
        let (ok, _, err) = git_seriate_env(&repo, &["settle", "--into", "main"], &env);
        assert!(ok, "{kind}: {err}");
        assert!(!asked.exists(), "{kind}: a passphrase was asked for");
        verify_all(&repo, "main", &key.with_extension("pub"));
    }
}

#[test]
fn an_encrypted_key_asks_once() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    let Some(key) = ssh_key(tmp.path(), "key", "ed25519", "secret words") else {
        eprintln!("skipped: no ssh-keygen");
        return;
    };
    sign_with(&repo, &key);
    small(&repo);
    let count = tmp.path().join("asked");
    let askpass = tmp.path().join("askpass");
    let script = |answer: &str| {
        format!(
            "#!/bin/sh\necho x >> '{}'\necho '{answer}'\n",
            count.display()
        )
    };
    use std::os::unix::fs::PermissionsExt;
    let env = [
        ("SSH_ASKPASS", askpass.to_str().unwrap()),
        ("SSH_ASKPASS_REQUIRE", "force"),
        // No ssh-keygen: the key is decrypted in-process.
        ("PATH", ""),
    ];

    // A wrong passphrase: refused, nothing written.
    std::fs::write(&askpass, script("wrong")).unwrap();
    std::fs::set_permissions(&askpass, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (ok, _, err) = git_seriate_env(&repo, &["settle", "--into", "main"], &env);
    assert!(!ok);
    assert!(err.contains("bad passphrase"), "{err}");
    assert!(git(&repo, &["branch", "--list", "main"]).is_empty());

    // The right one: asked for once, for every commit.
    std::fs::remove_file(&count).unwrap();
    std::fs::write(&askpass, script("secret words")).unwrap();
    let (ok, _, err) = git_seriate_env(&repo, &["settle", "--into", "main"], &env);
    assert!(ok, "{err}");
    assert!(
        git(&repo, &["rev-list", "--count", "main"])
            .trim()
            .parse::<u32>()
            .unwrap()
            > 1
    );
    assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1);
    verify_all(&repo, "main", &key.with_extension("pub"));
}

/// Runs the `git ...` command a checked-out note names, in `repo`.
fn run_note(repo: &Path, err: &str) -> String {
    let command = err
        .split('\'')
        .nth(1)
        .unwrap_or_else(|| panic!("no command in: {err}"));
    let words: Vec<&str> = command.split_whitespace().collect();
    assert_eq!(words[0], "git", "{command}");
    git(repo, &words[1..]);
    command.to_string()
}

/// Settling the checked-out branch leaves the index and work tree alone;
/// the command the note names brings them to the new tip, keeping local
/// changes, whether the branch was unborn or had a tip.
#[test]
fn the_checked_out_note_names_a_command_that_works() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("work");
    git(
        tmp.path(),
        &["init", "-q", "-b", "main", repo.to_str().unwrap()],
    );
    std::fs::write(repo.join("local.txt"), "mine\n").unwrap();
    small(&repo);

    // Unborn: no previous tip.
    let (ok, _, err) = git_seriate(&repo, &["settle"]);
    assert!(ok, "{err}");
    assert_eq!(run_note(&repo, &err), "git read-tree -m -u main");
    assert_eq!(git(&repo, &["status", "--porcelain"]), "?? local.txt\n");
    assert_eq!(
        std::fs::read_to_string(repo.join("f.txt")).unwrap(),
        "a\nb\nc\n"
    );

    // A previous tip, and an uncommitted edit to a file settle doesn't touch.
    git(&repo, &["config", "seriate.itemPattern", "f.txt"]);
    std::fs::write(repo.join("notes.md"), "notes\n").unwrap();
    git(&repo, &["add", "notes.md"]);
    git(&repo, &["commit", "-q", "-m", "Add notes"]);
    std::fs::write(repo.join("notes.md"), "notes, edited\n").unwrap();
    let r = gix::open(&repo).unwrap();
    store::add(
        &r,
        NewSnapshot {
            source: Source {
                identity: "s3".into(),
                build_time: 1_700_000_000 + 3 * 86_400,
                order_hint: None,
                group: None,
                metadata: Vec::new(),
            },
            items: vec![Item {
                path: "f.txt".into(),
                bytes: b"a\nb\nc\nd\n".to_vec(),
            }],
            rejected: Vec::new(),
            record: Vec::new(),
        },
    )
    .unwrap();
    let (ok, _, err) = git_seriate(&repo, &["settle"]);
    assert!(ok, "{err}");
    assert_eq!(run_note(&repo, &err), "git read-tree -m -u main-1 main");
    assert_eq!(
        git(&repo, &["status", "--porcelain"]),
        " M notes.md\n?? local.txt\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("f.txt")).unwrap(),
        "a\nb\nc\nd\n"
    );
}

/// `--stats` names each move of the sort: here s1, dated before s0 though
/// its file grew from s0's, moved by phase 3 between s0 and s2.
#[test]
fn stats_name_each_move_of_the_sort() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = new_repo(tmp.path());
    let r = gix::open(&repo).unwrap();
    for (id, day, text, group) in [
        ("s0", 1, "a\n", None),
        ("s1", 0, "a\nb\n", Some("g")),
        ("s2", 2, "a\nb\nc\n", None),
    ] {
        store::add(
            &r,
            NewSnapshot {
                source: Source {
                    identity: id.into(),
                    build_time: 1_700_000_000 + day * 86_400,
                    order_hint: None,
                    group: group.map(str::to_string),
                    metadata: Vec::new(),
                },
                items: vec![Item {
                    path: "f.txt".into(),
                    bytes: text.as_bytes().to_vec(),
                }],
                rejected: Vec::new(),
                record: Vec::new(),
            },
        )
        .unwrap();
    }
    let (ok, out, err) = git_seriate(&repo, &["settle", "--into", "main", "--dry-run", "--stats"]);
    assert!(ok, "{err}");
    assert!(out.contains("would settle main: kept 0, wrote 3"), "{out}");
    assert!(err.contains("sort: 3 snapshot(s)\n"), "{err}");
    assert!(
        err.contains("phase 1 build time: 2 position(s) changed\n"),
        "{err}"
    );
    assert!(err.contains("phase 2 order hints: skipped\n"), "{err}");
    assert!(
        err.contains("phase 3 containment: 1 move(s), 3 comparison(s)"),
        "{err}"
    );
    assert!(
        err.contains("  move s1 [g]: 0 -> 1, +1; now between s0 and s2\n"),
        "{err}"
    );
    assert!(err.contains("phase 4 interleaving: 0 move(s)"), "{err}");
    // Without --stats, none of it.
    let (ok, _, err) = git_seriate(&repo, &["settle", "--into", "main", "--dry-run"]);
    assert!(ok);
    assert!(!err.contains("phase"), "{err}");
}
