// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Synthetic corpora with known true orders (spec 0451 S5), for the
//! self-test and for comparing implementations.
//!
//! A corpus is a history of text files evolving over revisions, and
//! snapshots of it: each holds some of the files at one revision, with a
//! build time near that revision's and, usually, the revision as its order
//! hint. Everything is drawn from one seeded generator, so the same
//! parameters always give the same bytes.

use std::fmt::Write as _;
use std::io;
use std::path::Path;

/// SplitMix64: a few lines, and stable across versions of everything else,
/// so a committed corpus never moves with a dependency.
#[derive(Debug, Clone)]
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`, `n > 0`.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    /// True with probability `percent` / 100.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// Revision 0's time, 2023-11-14: far enough from 0 that no build time is
/// negative, which a git date cannot be.
pub const EPOCH: i64 = 1_700_000_000;

/// What to generate. `Default` is a small corpus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params {
    pub seed: u64,
    /// Files at revision 0.
    pub files: u32,
    pub revisions: u32,
    pub snapshots: u32,
    /// Seconds between revisions.
    pub revision_seconds: i64,
    /// A build time is its revision's, plus or minus up to this.
    pub build_noise_seconds: i64,
    /// Percent of the snapshots without an order hint.
    pub hint_missing_percent: u64,
    /// Percent of the hinted snapshots that carry the previous revision's
    /// hint instead of their own.
    pub hint_shared_percent: u64,
    /// Snapshots drawn per build: they share a revision and a build time.
    pub per_build: u32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            seed: 1,
            files: 12,
            revisions: 40,
            snapshots: 60,
            revision_seconds: 86_400,
            build_noise_seconds: 2 * 86_400,
            hint_missing_percent: 0,
            hint_shared_percent: 0,
            per_build: 1,
        }
    }
}

impl Params {
    /// The arguments of `seriation-gen` that give these parameters.
    pub fn to_args(&self) -> Vec<String> {
        vec![
            format!("--seed={}", self.seed),
            format!("--files={}", self.files),
            format!("--revisions={}", self.revisions),
            format!("--snapshots={}", self.snapshots),
            format!("--revision-seconds={}", self.revision_seconds),
            format!("--build-noise-seconds={}", self.build_noise_seconds),
            format!("--hint-missing-percent={}", self.hint_missing_percent),
            format!("--hint-shared-percent={}", self.hint_shared_percent),
            format!("--per-build={}", self.per_build),
        ]
    }

    /// Parse `--name=value` arguments over the defaults.
    pub fn from_args<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut p = Params::default();
        for arg in args {
            let (name, value) = arg
                .strip_prefix("--")
                .and_then(|a| a.split_once('='))
                .ok_or_else(|| format!("expected --name=value, got {arg:?}"))?;
            let bad = |e: std::num::ParseIntError| format!("--{name}: {e}");
            match name {
                "seed" => p.seed = value.parse().map_err(bad)?,
                "files" => p.files = value.parse().map_err(bad)?,
                "revisions" => p.revisions = value.parse().map_err(bad)?,
                "snapshots" => p.snapshots = value.parse().map_err(bad)?,
                "revision-seconds" => p.revision_seconds = value.parse().map_err(bad)?,
                "build-noise-seconds" => p.build_noise_seconds = value.parse().map_err(bad)?,
                "hint-missing-percent" => p.hint_missing_percent = value.parse().map_err(bad)?,
                "hint-shared-percent" => p.hint_shared_percent = value.parse().map_err(bad)?,
                "per-build" => p.per_build = value.parse().map_err(bad)?,
                _ => return Err(format!("unknown option --{name}")),
            }
        }
        if p.revisions == 0 || p.per_build == 0 {
            return Err("--revisions and --per-build must be at least 1".into());
        }
        if p.hint_missing_percent > 100 || p.hint_shared_percent > 100 {
            return Err("percentages are at most 100".into());
        }
        Ok(p)
    }
}

/// One snapshot of a generated corpus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedSnapshot {
    /// A valid git branch name.
    pub identity: String,
    pub build_time: i64,
    pub order_hint: Option<u64>,
    /// The truth.
    pub revision: u32,
    /// `(path, contents)`, by path.
    pub files: Vec<(String, String)>,
}

/// A generated corpus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corpus {
    pub params: Params,
    pub snapshots: Vec<GeneratedSnapshot>,
}

/// The history: each file's text at each revision, `None` when absent.
fn history(p: &Params, rng: &mut SplitMix64) -> Vec<(String, Vec<Option<String>>)> {
    let mut files: Vec<(String, Vec<Option<String>>)> = Vec::new();
    let mut texts: Vec<Option<Vec<String>>> = Vec::new();
    let mut previous: Vec<Option<Vec<String>>> = Vec::new();
    let new_file = |n: usize| -> (String, Vec<String>) {
        (
            format!("dir{}/file{n:03}.txt", n % 4),
            vec![format!("file {n}")],
        )
    };
    for n in 0..p.files as usize {
        let (path, text) = new_file(n);
        files.push((path, Vec::new()));
        texts.push(Some(text));
        previous.push(None);
    }
    for r in 0..p.revisions {
        if r > 0 {
            // A file now and then.
            if rng.chance(10) {
                let (path, text) = new_file(files.len());
                files.push((path, vec![None; r as usize]));
                texts.push(Some(text));
                previous.push(None);
            }
            for i in 0..texts.len() {
                let Some(lines) = texts[i].clone() else {
                    continue;
                };
                if !rng.chance(30) {
                    continue;
                }
                let mut next = lines.clone();
                match rng.below(100) {
                    // Append a line.
                    0..=39 => next.push(format!("rev {r} line {}", next.len())),
                    // Insert a line.
                    40..=64 => {
                        let at = rng.below(next.len() as u64 + 1) as usize;
                        next.insert(at, format!("rev {r} inserted"));
                    }
                    // Insert characters within a line.
                    65..=89 => {
                        let at = rng.below(next.len() as u64) as usize;
                        let line = &mut next[at];
                        let cut = line
                            .char_indices()
                            .nth(rng.below(line.chars().count() as u64 + 1) as usize)
                            .map_or(line.len(), |(i, _)| i);
                        line.insert_str(cut, &format!("[{r}]"));
                    }
                    // Go back to the previous version.
                    90..=96 => {
                        if let Some(prev) = previous[i].clone() {
                            next = prev;
                        }
                    }
                    // Delete the file.
                    _ => {
                        previous[i] = Some(lines);
                        texts[i] = None;
                        continue;
                    }
                }
                previous[i] = Some(lines);
                texts[i] = Some(next);
            }
        }
        for (i, file) in files.iter_mut().enumerate() {
            file.1.push(texts[i].as_ref().map(|l| l.join("\n") + "\n"));
        }
    }
    files
}

/// Generate a corpus.
pub fn generate(p: &Params) -> Corpus {
    let mut rng = SplitMix64(p.seed);
    let files = history(p, &mut rng);
    let mut snapshots = Vec::new();
    let builds = p.snapshots.div_ceil(p.per_build);
    for b in 0..builds {
        let revision = rng.below(u64::from(p.revisions)) as u32;
        let noise = if p.build_noise_seconds > 0 {
            rng.below(2 * p.build_noise_seconds as u64 + 1) as i64 - p.build_noise_seconds
        } else {
            0
        };
        let build_time = EPOCH + i64::from(revision) * p.revision_seconds + noise;
        let hint = if rng.chance(p.hint_missing_percent) {
            None
        } else if revision > 0 && rng.chance(p.hint_shared_percent) {
            Some(u64::from(revision) - 1)
        } else {
            Some(u64::from(revision))
        };
        for k in 0..p.per_build {
            if snapshots.len() == p.snapshots as usize {
                break;
            }
            let held: Vec<(String, String)> = files
                .iter()
                .filter_map(|(path, texts)| texts[revision as usize].clone().map(|t| (path, t)))
                .filter(|_| rng.chance(70))
                .map(|(path, t)| (path.clone(), t))
                .collect();
            snapshots.push(GeneratedSnapshot {
                identity: format!("build{b:04}/part{k}"),
                build_time,
                order_hint: hint,
                revision,
                files: held,
            });
        }
    }
    Corpus {
        params: p.clone(),
        snapshots,
    }
}

impl Corpus {
    /// The manifest: `{"command": […], "snapshots": [{identity, build_time,
    /// order_hint, revision}, …]}`, one snapshot per line.
    pub fn manifest(&self) -> String {
        let mut out = String::from("{\n  \"command\": [\"seriation-gen\"");
        for arg in self.params.to_args() {
            write!(out, ", \"{arg}\"").unwrap();
        }
        out.push_str("],\n  \"snapshots\": [\n");
        for (i, s) in self.snapshots.iter().enumerate() {
            let hint = s.order_hint.map_or("null".to_string(), |h| h.to_string());
            writeln!(
                out,
                "    {{\"identity\": \"{}\", \"build_time\": {}, \"order_hint\": {}, \"revision\": {}}}{}",
                s.identity,
                s.build_time,
                hint,
                s.revision,
                if i + 1 < self.snapshots.len() { "," } else { "" }
            )
            .unwrap();
        }
        out.push_str("  ]\n}\n");
        out
    }

    /// Write the corpus under `dir`: `manifest.json`, and one directory per
    /// snapshot, named by its identity, holding its files.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("manifest.json"), self.manifest())?;
        for s in &self.snapshots {
            for (path, text) in &s.files {
                let file = dir.join("snapshots").join(&s.identity).join(path);
                std::fs::create_dir_all(file.parent().unwrap())?;
                std::fs::write(file, text)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_parameters_give_the_same_corpus() {
        let p = Params::default();
        assert_eq!(generate(&p), generate(&p));
        let other = Params {
            seed: 2,
            ..p.clone()
        };
        assert_ne!(generate(&p), generate(&other));
    }

    #[test]
    fn arguments_round_trip() {
        let p = Params {
            seed: 9,
            per_build: 5,
            hint_missing_percent: 30,
            ..Params::default()
        };
        assert_eq!(Params::from_args(p.to_args()).unwrap(), p);
        assert!(Params::from_args(["--bogus=1".to_string()]).is_err());
    }

    #[test]
    fn builds_share_a_revision_and_a_build_time() {
        let corpus = generate(&Params {
            per_build: 4,
            snapshots: 12,
            ..Params::default()
        });
        assert_eq!(corpus.snapshots.len(), 12);
        for build in corpus.snapshots.chunks(4) {
            assert!(build
                .iter()
                .all(|s| s.revision == build[0].revision && s.build_time == build[0].build_time));
        }
    }
}
