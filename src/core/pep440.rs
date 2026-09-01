//! A PEP 440 version parser, comparator, and update classifier.
//!
//! Hand-rolled rather than delegated to `pep440_rs`, deliberately: this crate
//! ships no Python dependency today and one classifier is a smaller surface than
//! a new dependency tree. The consequence is that the *test table* is the real
//! contract — every worked example in `docs/python-schema.md#update-classification`
//! is asserted below, so swapping in a crate later is a contained change that has
//! to satisfy the same table.
//!
//! # Why the input grammar is wider than the normalized form
//!
//! PEP 440's normalized form is `[N!]N(.N)*[{a|b|rc}N][.postN][.devN][+local]`,
//! but the *accepted* form is much larger: `v1.0`, `1.0-1`, `1.0beta2`,
//! `1.0.alpha1`, `1.0-rc1`, `1.0rev1`, `1.0-dev`, `1.0preview1`, and `01.0` are
//! all valid and normalize to `1.0`, `1.0.post1`, `1.0b2`, `1.0a1`, `1.0rc1`,
//! `1.0.post1`, `1.0.dev0`, `1.0rc1`, and `1.0`.
//!
//! Treating the normalized grammar as the input grammar is the failure mode this
//! module is built to avoid, and it is an *invisible* one: an unparsed version
//! becomes [`PythonUpdateType::Unclassified`], which the schema defines as an
//! honest "we could not tell". A classifier that quietly gave up on a third of
//! its input would look exactly like one that worked.

use crate::core::python::PythonUpdateType;
use std::cmp::Ordering;

/// A parsed PEP 440 version, held in normalized form.
///
/// Construction goes through [`Version::parse`], so every value here has already
/// had its spelling variation collapsed: leading zeros stripped from release
/// components, `alpha`/`beta`/`c`/`pre`/`preview` folded onto `a`/`b`/`rc`,
/// `rev`/`r` and the bare `-N` suffix folded onto `post`, and omitted qualifier
/// numbers defaulted to zero.
#[derive(Debug, Clone)]
pub struct Version {
    epoch: u64,
    /// At least one component, each with leading zeros already stripped.
    release: Vec<u64>,
    pre: Option<(PreLabel, u64)>,
    post: Option<u64>,
    dev: Option<u64>,
    local: Option<Vec<LocalSegment>>,
}

/// The three pre-release kinds PEP 440 normalizes to, in release order.
///
/// The derived `Ord` is the PEP 440 order — alpha before beta before release
/// candidate — so the variants must stay in this sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PreLabel {
    A,
    B,
    Rc,
}

/// One dot-separated piece of a local version label.
///
/// PEP 440 orders numeric segments above alphanumeric ones, which is the derived
/// variant order here.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum LocalSegment {
    Alphanumeric(String),
    Numeric(u64),
}

/// The tuple PEP 440 ordering and equality both reduce to.
///
/// A tuple rather than a hand-written comparator so that `Ord` and `Eq` cannot
/// drift apart: `1.0` and `1.0.0` are the *same version*, and an `Ord` that said
/// so while `Eq` disagreed would violate the trait's contract as well as the
/// spec. Both go through this.
type SortKey = (
    u64,
    Vec<u64>,
    Sentinel<(PreLabel, u64)>,
    Sentinel<u64>,
    Sentinel<u64>,
    Sentinel<Vec<LocalSegment>>,
);

/// A comparison key that may sort below or above every concrete value.
///
/// PEP 440 needs both sentinels in the same key: an absent post-release sorts
/// *below* every post-release, while an absent dev-release sorts *above* every
/// dev-release. The derived `Ord` gives `Below < Exact(..) < Above`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Sentinel<T> {
    Below,
    Exact(T),
    Above,
}

/// The pre-release labels, longest first.
///
/// Order is load-bearing rather than cosmetic. `preview` has to be tried before
/// `pre`, or `1.0preview1` matches `pre` and leaves `view1`, which no later
/// segment can consume — so a perfectly valid version fails to parse and silently
/// becomes `unclassified`.
const PRE_LABELS: [(&str, PreLabel); 8] = [
    ("preview", PreLabel::Rc),
    ("alpha", PreLabel::A),
    ("beta", PreLabel::B),
    ("pre", PreLabel::Rc),
    ("rc", PreLabel::Rc),
    ("a", PreLabel::A),
    ("b", PreLabel::B),
    ("c", PreLabel::Rc),
];

/// The post-release labels, longest first, for the same reason [`PRE_LABELS`] is
/// ordered: `1.0rev1` must match `rev` rather than `r`, which would leave `ev1`.
const POST_LABELS: [&str; 3] = ["post", "rev", "r"];

impl Version {
    /// Parses the full PEP 440 version scheme, returning `None` for anything
    /// outside it.
    ///
    /// `None` is the honest outcome rather than an error: the caller turns it
    /// into [`PythonUpdateType::Unclassified`], which the schema defines as "we
    /// could not tell" instead of a defaulted guess.
    pub fn parse(text: &str) -> Option<Self> {
        // PEP 440 permits surrounding whitespace and is case-insensitive.
        let lowered = text.trim().to_ascii_lowercase();
        let mut rest = lowered.as_str();

        // The `v` prefix is accepted and dropped, so `v1.0` is `1.0`.
        rest = rest.strip_prefix('v').unwrap_or(rest);

        let epoch = match rest.split_once('!') {
            Some((epoch, remainder)) => {
                rest = remainder;
                parse_number(epoch)?
            }
            None => 0,
        };

        let (release, remainder) = parse_release(rest)?;
        rest = remainder;

        let (pre, remainder) = parse_pre(rest);
        rest = remainder;

        let (post, remainder) = parse_post(rest);
        rest = remainder;

        let (dev, remainder) = parse_dev(rest);
        rest = remainder;

        let local = match rest.strip_prefix('+') {
            Some(label) => {
                rest = "";
                Some(parse_local(label)?)
            }
            None => None,
        };

        // Anything left over means the string was not a version with a suffix we
        // ignored; it was simply not a version. Accepting a trailing remainder is
        // how a classifier starts reporting confident nonsense.
        if !rest.is_empty() {
            return None;
        }

        Some(Version {
            epoch,
            release,
            pre,
            post,
            dev,
            local,
        })
    }

    /// The PEP 440 sort key, with the release segment's trailing zeros stripped
    /// so that `1.0` and `1.0.0` compare equal.
    fn sort_key(&self) -> SortKey {
        let mut release = self.release.clone();
        while release.len() > 1 && release.last() == Some(&0) {
            release.pop();
        }

        // A dev release with no pre- or post-segment precedes every other
        // spelling of the same release, including the release itself.
        let pre = match (self.pre, self.post, self.dev) {
            (Some(pre), _, _) => Sentinel::Exact(pre),
            (None, None, Some(_)) => Sentinel::Below,
            (None, _, _) => Sentinel::Above,
        };

        (
            self.epoch,
            release,
            pre,
            self.post.map_or(Sentinel::Below, Sentinel::Exact),
            self.dev.map_or(Sentinel::Above, Sentinel::Exact),
            self.local.clone().map_or(Sentinel::Below, Sentinel::Exact),
        )
    }
}

/// Equality is PEP 440 equality, not field-by-field equality.
///
/// Derived equality would compare the stored release verbatim and report `1.0`
/// and `1.0.0` as different versions. The schema calls that pair `unclassified`
/// precisely because they are *equal*, so the comparison has to go through the
/// same sort key `Ord` uses — an `Ord` that disagreed with `Eq` would be a
/// contract violation as well as the wrong answer.
impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.sort_key() == other.sort_key()
    }
}

impl Eq for Version {}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Classifies how `latest` differs from `current`.
///
/// Both strings are normalized before anything is compared, which is the whole
/// point of the exercise — see the module documentation.
///
/// Two cases collapse to [`PythonUpdateType::Unclassified`] on purpose. An
/// unparseable version is the obvious one. The other is a `latest` that is
/// *older* than `current`, which Poetry reports when the newest stable release is
/// behind an installed pre-release: calling that a `major` update would advertise
/// a downgrade as an available upgrade.
/// Whether two version strings denote the same PEP 440 version.
///
/// Not string equality: `1.0`, `1.0.0`, and `01.0` are all the same version, and
/// a caller that compares the raw strings would report an update nobody can act
/// on. Unparseable input is never "the same", so an unrecognized version stays
/// visible rather than being silently filtered away.
pub fn is_same_version(left: &str, right: &str) -> bool {
    match (Version::parse(left), Version::parse(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

pub fn classify(current: &str, latest: &str) -> PythonUpdateType {
    let (Some(current), Some(latest)) = (Version::parse(current), Version::parse(latest)) else {
        return PythonUpdateType::Unclassified;
    };

    if latest < current {
        return PythonUpdateType::Unclassified;
    }

    if current.epoch != latest.epoch {
        return PythonUpdateType::Epoch;
    }

    // Release segments are zero-padded to equal length before comparison, so
    // `1.4` to `1.4.1` is a patch rather than a change of arity.
    let width = current.release.len().max(latest.release.len());
    for index in 0..width {
        if release_component(&current, index) != release_component(&latest, index) {
            return match index {
                0 => PythonUpdateType::Major,
                1 => PythonUpdateType::Minor,
                _ => PythonUpdateType::Patch,
            };
        }
    }

    if current == latest {
        // Identical versions, spelled differently or not. There is no difference
        // to classify, so claiming one would be an invention.
        PythonUpdateType::Unclassified
    } else {
        PythonUpdateType::Qualifier
    }
}

/// One zero-padded release component, so two versions of different arity can be
/// compared position by position.
fn release_component(version: &Version, index: usize) -> u64 {
    version.release.get(index).copied().unwrap_or(0)
}

/// Parses `N(.N)*`, returning the components and whatever follows them.
///
/// A dot is consumed only when a digit follows it, because `.` is also the
/// optional separator in front of every qualifier. Reading the release as one
/// greedy run of digits and dots swallowed the separator in `0.9.0.post1`,
/// `1.0.alpha1`, and `1.0.dev1` — all documented spellings — and left an empty
/// final component that failed to parse, so three valid versions came back
/// `unclassified`.
fn parse_release(text: &str) -> Option<(Vec<u64>, &str)> {
    let mut release = Vec::new();
    let mut rest = text;

    loop {
        let end = rest
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(rest.len());
        release.push(parse_number(&rest[..end])?);
        rest = &rest[end..];

        let follows_a_digit = rest
            .strip_prefix('.')
            .is_some_and(|next| next.starts_with(|character: char| character.is_ascii_digit()));
        if !follows_a_digit {
            return Some((release, rest));
        }
        rest = &rest[1..];
    }
}

/// Parses `[-_.]?(alpha|a|beta|b|c|pre|preview|rc)[-_.]?N?`.
fn parse_pre(text: &str) -> (Option<(PreLabel, u64)>, &str) {
    let after_separator = strip_separator(text);
    for (spelling, label) in PRE_LABELS {
        if let Some(rest) = after_separator.strip_prefix(spelling) {
            let (number, rest) = parse_optional_number(rest);
            return (Some((label, number)), rest);
        }
    }
    (None, text)
}

/// Parses either `-N` or `[-_.]?(post|rev|r)[-_.]?N?`.
///
/// The bare `-N` form is the one that reads as an accident: `1.0-1` is a valid
/// PEP 440 version normalizing to `1.0.post1`, and a parser that only knows the
/// spelled form rejects it.
fn parse_post(text: &str) -> (Option<u64>, &str) {
    if let Some(rest) = text.strip_prefix('-') {
        let digits = rest
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits > 0 {
            if let Some(number) = parse_number(&rest[..digits]) {
                return (Some(number), &rest[digits..]);
            }
        }
    }

    let after_separator = strip_separator(text);
    for spelling in POST_LABELS {
        if let Some(rest) = after_separator.strip_prefix(spelling) {
            let (number, rest) = parse_optional_number(rest);
            return (Some(number), rest);
        }
    }
    (None, text)
}

/// Parses `[-_.]?dev[-_.]?N?`.
fn parse_dev(text: &str) -> (Option<u64>, &str) {
    let after_separator = strip_separator(text);
    match after_separator.strip_prefix("dev") {
        Some(rest) => {
            let (number, rest) = parse_optional_number(rest);
            (Some(number), rest)
        }
        None => (None, text),
    }
}

/// Parses the local label after `+`: alphanumeric runs joined by `-`, `_`, or `.`.
fn parse_local(text: &str) -> Option<Vec<LocalSegment>> {
    if text.is_empty() {
        return None;
    }

    let mut segments = Vec::new();
    for segment in text.split(['-', '_', '.']) {
        if segment.is_empty() || !segment.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        segments.push(match segment.parse::<u64>() {
            Ok(number) => LocalSegment::Numeric(number),
            Err(_) => LocalSegment::Alphanumeric(segment.to_string()),
        });
    }
    Some(segments)
}

/// Consumes one optional `[-_.]` separator.
fn strip_separator(text: &str) -> &str {
    text.strip_prefix(['-', '_', '.']).unwrap_or(text)
}

/// Reads an optional `[-_.]?N` suffix, defaulting an omitted number to zero.
///
/// PEP 440 spells `1.0a` and `1.0a0` the same way, so the default is part of the
/// normalization rather than a convenience.
fn parse_optional_number(text: &str) -> (u64, &str) {
    let after_separator = strip_separator(text);
    let end = after_separator
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(after_separator.len());
    match parse_number(&after_separator[..end]) {
        Some(number) => (number, &after_separator[end..]),
        // No digits, but the separator is still consumed. PEP 440's regex places
        // the optional `[-_.]` *before* the optional digits and does not
        // backtrack it, so `1.0a.` and `1.0.dev-` are valid. Handing back the
        // original text here would leave that separator for the next segment,
        // which has only one optional separator of its own to spend, and the
        // parse would fail on a version `packaging` accepts.
        None => (0, after_separator),
    }
}

/// Parses a non-empty run of ASCII digits, tolerating leading zeros.
///
/// `01.0` is a valid PEP 440 version equal to `1.0`, so the zeros are stripped
/// here rather than rejected.
fn parse_number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders a parsed version back into PEP 440 normalized form.
    ///
    /// Test-only, and deliberately so: nothing in the payload carries a
    /// normalized version — the schema reports `current` and `latest` as the
    /// source gave them. This exists to make the normalization table below
    /// assert on the parse *result* rather than on a classification that could
    /// agree for the wrong reason.
    fn normalized(version: &Version) -> String {
        let mut rendered = String::new();
        if version.epoch != 0 {
            rendered.push_str(&format!("{}!", version.epoch));
        }
        rendered.push_str(
            &version
                .release
                .iter()
                .map(|component| component.to_string())
                .collect::<Vec<_>>()
                .join("."),
        );
        if let Some((label, number)) = version.pre {
            let label = match label {
                PreLabel::A => "a",
                PreLabel::B => "b",
                PreLabel::Rc => "rc",
            };
            rendered.push_str(&format!("{label}{number}"));
        }
        if let Some(post) = version.post {
            rendered.push_str(&format!(".post{post}"));
        }
        if let Some(dev) = version.dev {
            rendered.push_str(&format!(".dev{dev}"));
        }
        if let Some(local) = &version.local {
            let rendered_local = local
                .iter()
                .map(|segment| match segment {
                    LocalSegment::Numeric(number) => number.to_string(),
                    LocalSegment::Alphanumeric(text) => text.clone(),
                })
                .collect::<Vec<_>>()
                .join(".");
            rendered.push_str(&format!("+{rendered_local}"));
        }
        rendered
    }

    /// The spelling variations `docs/python-schema.md` names, plus the rest of
    /// PEP 440's accepted grammar.
    ///
    /// This table is the deliverable. Every entry is a version that is *valid*
    /// but is not written in normalized form, which is exactly the input a
    /// classifier written against the normalized grammar silently rejects.
    #[test]
    fn accepted_spellings_normalize() {
        for (input, expected) in [
            // The nine the schema documents by name.
            ("v1.0", "1.0"),
            ("1.0-1", "1.0.post1"),
            ("1.0beta2", "1.0b2"),
            ("1.0.alpha1", "1.0a1"),
            ("1.0-rc1", "1.0rc1"),
            ("1.0rev1", "1.0.post1"),
            ("1.0-dev", "1.0.dev0"),
            ("1.0preview1", "1.0rc1"),
            ("01.0", "1.0"),
            // A qualifier separator with the number omitted. PEP 440's regex puts
            // the optional `[-_.]` before the optional digits and does not
            // backtrack it, so all of these are valid and the number defaults to
            // zero. These were rejected until the separator was consumed even
            // when no digits followed; the failure was invisible because a
            // rejected version classifies as `unclassified`, which the schema
            // defines as an honest "we could not tell".
            ("1.0a.", "1.0a0"),
            ("1.0a-", "1.0a0"),
            ("1.0a_", "1.0a0"),
            ("1.0.post-", "1.0.post0"),
            ("1.0.dev.", "1.0.dev0"),
            ("1.0rev.", "1.0.post0"),
            ("1.0-rc-", "1.0rc0"),
            ("1.0a-.dev1", "1.0a0.dev1"),
            ("1.0a..dev1", "1.0a0.dev1"),
            ("1.0a.+local", "1.0a0+local"),
            // The rest of the accepted grammar.
            ("1.0c1", "1.0rc1"),
            ("1.0r1", "1.0.post1"),
            ("1.0.post1", "1.0.post1"),
            ("1.0_post_1", "1.0.post1"),
            ("1.0-post-1", "1.0.post1"),
            ("1.0.post", "1.0.post0"),
            ("1.0a", "1.0a0"),
            ("1.0alpha", "1.0a0"),
            ("1.0.dev", "1.0.dev0"),
            ("1.0_dev_2", "1.0.dev2"),
            ("1!2.0", "1!2.0"),
            ("  1.0  ", "1.0"),
            ("V1.0", "1.0"),
            ("1.0A1", "1.0a1"),
            ("1.0.0.0.1", "1.0.0.0.1"),
            ("0001.0002", "1.2"),
            ("1.0+ubuntu.1", "1.0+ubuntu.1"),
            ("1.0+UBUNTU_1", "1.0+ubuntu.1"),
            ("1.0a1.post2.dev3+local.7", "1.0a1.post2.dev3+local.7"),
            ("2026.4", "2026.4"),
        ] {
            let parsed = Version::parse(input)
                .unwrap_or_else(|| panic!("{input} is a valid PEP 440 version but did not parse"));
            assert_eq!(
                normalized(&parsed),
                expected,
                "{input} normalized to the wrong version"
            );
        }
    }

    /// Strings outside the scheme must not parse into something plausible.
    ///
    /// A parser that accepts a leading garbage prefix, or ignores a trailing
    /// remainder, produces a confident classification of a version that was
    /// never there — worse than the `unclassified` a rejection produces.
    #[test]
    fn rejected_spellings_do_not_parse() {
        for input in [
            "",
            "not-a-version",
            "1.0.",
            ".1.0",
            "1..0",
            "1.0-",
            "1.0betaX",
            "1.0.dev1extra",
            "1.0+",
            "1.0+local..1",
            "1.0+local-",
            "1.0++1",
            "a1.0",
            "1.0!2.0",
            "!1.0",
            "1.0 rc1",
            "1.0+bad$char",
        ] {
            assert!(
                Version::parse(input).is_none(),
                "{input} is not a PEP 440 version but parsed anyway"
            );
        }
    }

    /// PEP 440's ordering rules, which the downgrade guard in [`classify`]
    /// depends on.
    ///
    /// Each row must hold strictly, so a comparator that collapsed any of these
    /// to equality would fail rather than pass by accident.
    #[test]
    fn ordering_follows_pep_440() {
        for (lower, higher) in [
            ("1.0.dev1", "1.0a1"),
            ("1.0a1", "1.0a2"),
            ("1.0a2", "1.0b1"),
            ("1.0b1", "1.0rc1"),
            ("1.0rc1", "1.0"),
            ("1.0", "1.0.post1"),
            ("1.0.post1", "1.0.post2"),
            ("1.0.post1", "1.1"),
            ("1.0", "1.0+local"),
            ("1.0+1", "1.0+2"),
            ("1.0+abc", "1.0+1"),
            ("1.0", "1!0.1"),
            ("1.9", "1.10"),
            ("1.0a1.dev1", "1.0a1"),
            ("1.0.post1.dev1", "1.0.post1"),
        ] {
            let lower_version = Version::parse(lower).expect(lower);
            let higher_version = Version::parse(higher).expect(higher);
            assert!(
                lower_version < higher_version,
                "{lower} should sort below {higher}"
            );
        }

        // Equality across spellings, which is what makes `1.0` vs `1.0.0`
        // `unclassified` rather than a patch.
        for (left, right) in [("1.0", "1.0.0"), ("1.0", "1.0.0.0"), ("1.0.0", "v1.0")] {
            let left_version = Version::parse(left).expect(left);
            let right_version = Version::parse(right).expect(right);
            assert!(
                left_version == right_version,
                "{left} and {right} are the same PEP 440 version"
            );
            assert_eq!(left_version.cmp(&right_version), Ordering::Equal);
        }
    }

    /// Every worked example in `docs/python-schema.md#update-classification`,
    /// plus the cases the prose calls out around them.
    #[test]
    fn documented_classification_examples() {
        for (current, latest, expected) in [
            // The eleven rows of the documented table, in order.
            ("1.4.2", "2.0.0", PythonUpdateType::Major),
            ("1.4.2", "1.5.0", PythonUpdateType::Minor),
            ("1.4.2", "1.4.3", PythonUpdateType::Patch),
            ("1.4", "1.4.1", PythonUpdateType::Patch),
            ("0.9.0", "0.9.0.post1", PythonUpdateType::Qualifier),
            ("2.0.0rc1", "2.0.0", PythonUpdateType::Qualifier),
            ("1.0", "1!1.0", PythonUpdateType::Epoch),
            ("2026.4", "2026.9", PythonUpdateType::Minor),
            ("1.0", "not-a-version", PythonUpdateType::Unclassified),
            ("v1.0", "1.0-1", PythonUpdateType::Qualifier),
            ("1.0", "1.0.0", PythonUpdateType::Unclassified),
            // An unparseable `current` is as unclassifiable as an unparseable
            // `latest`; the table only shows the latter.
            ("not-a-version", "1.0", PythonUpdateType::Unclassified),
            // The downgrade rule the prose states below the table.
            ("2.0.0", "1.9.9", PythonUpdateType::Unclassified),
            ("1.0.0rc1", "0.9.0", PythonUpdateType::Unclassified),
            ("1!1.0", "1.0", PythonUpdateType::Unclassified),
            // A fourth release component differs: still a patch, because the
            // rule is "first two equal, a later one differs".
            ("1.4.2.1", "1.4.2.2", PythonUpdateType::Patch),
            // Zero-padding works in both directions.
            ("1.4.0", "1.4", PythonUpdateType::Unclassified),
            ("1", "2", PythonUpdateType::Major),
            ("1", "1.1", PythonUpdateType::Minor),
            // Local segments are qualifiers, not patches.
            ("1.0", "1.0+build.1", PythonUpdateType::Qualifier),
            // A dev release is a qualifier difference too.
            ("1.0.dev1", "1.0", PythonUpdateType::Qualifier),
            // An epoch difference outranks a release difference.
            ("1.4.2", "1!0.1", PythonUpdateType::Epoch),
        ] {
            assert_eq!(
                classify(current, latest),
                expected,
                "{current} -> {latest} classified wrongly"
            );
        }
    }

    /// `major` is a statement about position, not about breakage.
    ///
    /// The schema is explicit that a calendar-versioned project bumping its
    /// first component is `major` and its second is `minor`, with no CalVer
    /// detection attempted. Pinned here so a later "improvement" that guesses at
    /// CalVer has to change a test that says why it must not.
    #[test]
    fn calendar_versions_are_classified_on_position_alone() {
        assert_eq!(classify("2025.12", "2026.1"), PythonUpdateType::Major);
        assert_eq!(classify("2026.4", "2026.9"), PythonUpdateType::Minor);
    }
}
