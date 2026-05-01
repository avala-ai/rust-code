//! Extract the file targets of in-place `sed` invocations.
//!
//! When the model runs `sed -i ...` (BSD or GNU style) it is performing
//! a file edit through the Bash tool. We want to route those edits
//! through the same permission path as [`crate::tools::file_edit`] so
//! that protected directories cannot be silently modified.
//!
//! This module's job is the parsing piece: given a [`ParsedCommand`],
//! figure out which file paths a `sed -i` invocation would touch.

use std::path::PathBuf;

use crate::tools::bash_parse::ParsedCommand;

/// Result of parsing a `sed -i` invocation out of a shell command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SedEdit {
    /// Files the `sed -i` invocation would write to. Relative paths are
    /// preserved verbatim — callers should resolve them against the
    /// session cwd before doing path-prefix checks.
    pub files: Vec<PathBuf>,
}

/// Find every in-place `sed` edit in `cmd` and return the set of files
/// it would touch. Returns an empty list when there is no `sed -i`.
///
/// Walks each pipeline / chained segment of the original command,
/// because tree-sitter only gives us command names — it does not
/// preserve the original argument order in [`ParsedCommand`]. Argument
/// parsing therefore happens on the raw segment string.
pub fn parse_sed_edits(cmd: &ParsedCommand) -> Vec<SedEdit> {
    let mut edits = Vec::new();

    if !cmd.commands.iter().any(|c| base(c) == "sed") {
        return edits;
    }

    for segment in segments(&cmd.raw) {
        let trimmed = segment.trim();
        if let Some(edit) = parse_sed_segment(trimmed) {
            edits.push(edit);
        }
    }

    edits
}

/// Parse a single shell segment that begins with `sed`.
///
/// Returns `Some` only when `-i` (or `--in-place`) is present and at
/// least one file argument follows the script.
pub fn parse_sed_segment(segment: &str) -> Option<SedEdit> {
    let tokens = tokenize(segment);
    let mut iter = tokens.iter().peekable();

    // Skip leading variable assignments like `LC_ALL=C sed ...`.
    while let Some(tok) = iter.peek() {
        if tok.contains('=') && !tok.starts_with('-') {
            iter.next();
        } else {
            break;
        }
    }

    let head = iter.next()?;
    if base(head) != "sed" {
        return None;
    }

    let mut in_place = false;
    let mut script_consumed = false;
    let mut files: Vec<PathBuf> = Vec::new();

    while let Some(tok) = iter.next() {
        if tok == "--" {
            // After `--`, the first remaining positional is the script
            // (unless one was already supplied via -e/-f) and every
            // subsequent positional is a file.
            for rest in iter.by_ref() {
                if !script_consumed {
                    script_consumed = true;
                    continue;
                }
                files.push(PathBuf::from(rest));
            }
            break;
        }
        if tok == "--in-place" {
            in_place = true;
            continue;
        }
        if let Some(stripped) = tok.strip_prefix("--in-place=") {
            in_place = true;
            // GNU sed accepts a backup-suffix argument here; ignore it.
            let _ = stripped;
            continue;
        }
        if tok == "-e" || tok == "--expression" || tok == "-f" || tok == "--file" {
            // Skip the script/file argument that follows.
            iter.next();
            script_consumed = true;
            continue;
        }
        // Long-option attached forms: `--expression=...` / `--file=...`.
        // The script/filename is glued to the flag with `=`; no
        // following positional should be consumed as the script.
        if tok.starts_with("--expression=") || tok.starts_with("--file=") {
            script_consumed = true;
            continue;
        }
        // Short-option attached forms: `-eSCRIPT` / `-fFILE`. These
        // must be detected BEFORE the generic short-cluster decomposer
        // — otherwise `-eSCRIPT` would be interpreted as a cluster of
        // boolean flags `e`, `S`, `C`, ... and the script characters
        // would silently be discarded.
        if let Some(rest) = tok.strip_prefix("-e")
            && !rest.is_empty()
        {
            script_consumed = true;
            continue;
        }
        if let Some(rest) = tok.strip_prefix("-f")
            && !rest.is_empty()
        {
            script_consumed = true;
            continue;
        }
        if let Some(parsed) = parse_short_option_cluster(tok) {
            if parsed.has_inplace {
                in_place = true;
                // BSD sed: `-i` (clustered or not) without an inline
                // suffix accepts the next token as a separate suffix
                // argument. Consume it only when it looks like one so
                // we do not eat the script.
                if !parsed.inplace_has_inline_suffix
                    && let Some(next) = iter.peek()
                    && looks_like_backup_suffix(next)
                {
                    iter.next();
                }
            }
            continue;
        }
        if tok.starts_with('-') {
            // Unrecognized long option (e.g. `--posix`, `--debug`) —
            // ignore. We deliberately do not return here; the goal is
            // to be permissive about unknown flags so we still detect
            // the file targets when `-i` is present.
            continue;
        }

        // First positional after options: the script. Subsequent
        // positionals are filenames.
        if !script_consumed {
            script_consumed = true;
            continue;
        }
        files.push(PathBuf::from(tok.as_str()));
    }

    if !in_place {
        return None;
    }
    if files.is_empty() {
        return None;
    }
    Some(SedEdit { files })
}

/// Outcome of decomposing a short-option token like `-Ei` or `-iSUFFIX`.
struct ShortClusterParse {
    /// Token contained an `i` character → in-place edit.
    has_inplace: bool,
    /// The cluster carried an inline suffix (GNU `-iSUFFIX` form, or a
    /// trailing run after `i` like `-Eie`). When false, BSD-style
    /// callers may still consume the next token as a separate suffix.
    inplace_has_inline_suffix: bool,
}

/// Parse a short-option cluster such as `-n`, `-Ei`, `-iSUFFIX`, or
/// `-nEi`. Returns `None` for anything that is not a single-dash short
/// cluster (long options or non-options).
///
/// We walk the characters left-to-right. When we encounter `i`, every
/// remaining character in the same token is the GNU inline backup
/// suffix and parsing stops. This matches GNU sed's documented
/// behaviour: `-i` may be clustered with other short flags as long as
/// it appears last, and any trailing characters become the suffix.
fn parse_short_option_cluster(tok: &str) -> Option<ShortClusterParse> {
    if tok.starts_with("--") {
        return None;
    }
    let rest = tok.strip_prefix('-')?;
    if rest.is_empty() {
        return None;
    }

    let mut has_inplace = false;
    let mut inplace_has_inline_suffix = false;

    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        if c == 'i' {
            has_inplace = true;
            // Anything left in the cluster is the GNU `-iSUFFIX`
            // inline backup suffix.
            if chars.next().is_some() {
                inplace_has_inline_suffix = true;
            }
            break;
        }
        // Other short flags are booleans for our purposes. We accept
        // unknown letters silently rather than bailing — the goal is
        // to find `i` whenever it appears.
    }

    Some(ShortClusterParse {
        has_inplace,
        inplace_has_inline_suffix,
    })
}

/// Strip surrounding quotes and a leading path from a command name.
fn base(raw: &str) -> &str {
    let trimmed = raw.trim_matches(|c: char| c == '"' || c == '\'');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// Does `tok` look like a BSD `sed -i` backup-suffix argument? The
/// heuristic matches an empty string, an extension-like token (`.bak`),
/// or any word that does not look like a sed script.
fn looks_like_backup_suffix(tok: &str) -> bool {
    if tok.is_empty() || tok == "''" || tok == "\"\"" {
        return true;
    }
    if tok.starts_with('-') {
        return false;
    }
    if tok.starts_with('s')
        || tok.starts_with('/')
        || tok.starts_with('y')
        || tok.starts_with('p')
        || tok.starts_with('d')
        || tok.starts_with('g')
    {
        return false;
    }
    // A short token starting with `.` (like `.bak`) is the canonical
    // BSD form.
    tok.starts_with('.') && tok.len() <= 8
}

/// Split a shell command on `|`, `&&`, `||`, and `;` boundaries while
/// preserving quoted runs verbatim.
fn segments(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            current.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                current.push(c);
            }
            '|' if chars.peek() == Some(&'|') => {
                chars.next();
                push_segment(&mut out, &mut current);
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                push_segment(&mut out, &mut current);
            }
            '|' | ';' => push_segment(&mut out, &mut current),
            _ => current.push(c),
        }
    }
    push_segment(&mut out, &mut current);
    out
}

fn push_segment(out: &mut Vec<String>, current: &mut String) {
    let s = current.trim().to_string();
    if !s.is_empty() {
        out.push(s);
    }
    current.clear();
}

/// Minimal POSIX-style tokenizer: splits on whitespace while keeping
/// quoted runs together. Good enough for the structural arguments
/// we care about (file paths and option flags); does not attempt to
/// honour shell escapes inside quoted strings.
fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for c in input.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                current.push(c);
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            ' ' | '\t' | '\n' => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::bash_parse::parse_bash;

    fn parse(s: &str) -> ParsedCommand {
        parse_bash(s).unwrap()
    }

    #[test]
    fn no_sed_returns_empty() {
        assert!(parse_sed_edits(&parse("ls -la")).is_empty());
    }

    #[test]
    fn sed_without_inplace_returns_empty() {
        assert!(parse_sed_edits(&parse("sed 's/foo/bar/' input.txt")).is_empty());
    }

    #[test]
    fn gnu_inplace_extracts_file() {
        let edits = parse_sed_edits(&parse("sed -i 's/foo/bar/' src/main.rs"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("src/main.rs")]);
    }

    #[test]
    fn long_inplace_extracts_file() {
        let edits = parse_sed_edits(&parse("sed --in-place 's/foo/bar/' README.md"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("README.md")]);
    }

    #[test]
    fn bsd_inplace_with_backup_suffix() {
        let edits = parse_sed_edits(&parse("sed -i .bak 's/foo/bar/' file.txt"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("file.txt")]);
    }

    #[test]
    fn multiple_files() {
        let edits = parse_sed_edits(&parse("sed -i 's/x/y/' a.txt b.txt c.txt"));
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].files,
            vec![
                PathBuf::from("a.txt"),
                PathBuf::from("b.txt"),
                PathBuf::from("c.txt"),
            ]
        );
    }

    #[test]
    fn chained_sed_each_segment_parsed() {
        let edits = parse_sed_edits(&parse(
            "sed -i 's/a/b/' first.txt && sed -i 's/c/d/' second.txt",
        ));
        assert_eq!(edits.len(), 2);
    }

    #[test]
    fn dash_e_script_is_not_treated_as_file() {
        let edits = parse_sed_edits(&parse("sed -i -e 's/foo/bar/' src/lib.rs"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("src/lib.rs")]);
    }

    #[test]
    fn double_dash_terminates_options() {
        let edits = parse_sed_edits(&parse("sed -i -- 's/x/y/' --weird-name.txt"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("--weird-name.txt")]);
    }

    #[test]
    fn gnu_inline_suffix_extracts_file() {
        // `-iSUFFIX` form (GNU): the suffix is glued to `i`.
        let edits = parse_sed_edits(&parse("sed -i.bak 's/foo/bar/' src/main.rs"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("src/main.rs")]);
    }

    #[test]
    fn clustered_extended_then_inplace() {
        // `-Ei` — extended regex + in-place. Was previously dropped by
        // the catch-all `tok.starts_with('-')` arm.
        let edits = parse_sed_edits(&parse("sed -Ei 's/x/y/' .git/config"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from(".git/config")]);
    }

    #[test]
    fn clustered_quiet_extended_inplace() {
        // `-nEi` — three short flags clustered, in-place last.
        let edits = parse_sed_edits(&parse("sed -nEi 's/x/y/' src/lib.rs"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("src/lib.rs")]);
    }

    #[test]
    fn clustered_inline_suffix() {
        // `-Eie` — extended + in-place with inline suffix `e`. The
        // following token must NOT be eaten as a separate suffix.
        let edits = parse_sed_edits(&parse("sed -Eie 's/x/y/' file.txt"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("file.txt")]);
    }

    #[test]
    fn bsd_clustered_inplace_separator_suffix() {
        // BSD-style: `-ie '.bak' file` — `-ie` is the cluster, `.bak`
        // is consumed as the separate backup suffix in some seds.
        // Either interpretation must still produce `file` as the
        // target.
        let edits = parse_sed_edits(&parse("sed -ie '.bak' file.txt"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("file.txt")]);
    }

    #[test]
    fn bsd_inplace_with_separator_suffix_and_extended() {
        // `-Ei .bak 's/x/y/' file` — clustered `-Ei` followed by a
        // BSD-style separate suffix.
        let edits = parse_sed_edits(&parse("sed -Ei .bak 's/x/y/' file.txt"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("file.txt")]);
    }

    #[test]
    fn long_inplace_with_value() {
        let edits = parse_sed_edits(&parse("sed --in-place=.bak 's/x/y/' file.txt"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("file.txt")]);
    }

    #[test]
    fn cluster_without_i_does_not_trigger() {
        // `-nE` has no `i` — must not be treated as in-place.
        assert!(parse_sed_edits(&parse("sed -nE 's/x/y/' file.txt")).is_empty());
    }

    #[test]
    fn long_inplace_with_value_and_long_expression_with_value() {
        // `--in-place=.bak` paired with `--expression=...`. Neither the
        // backup suffix nor the script is a file argument — only the
        // trailing positional is.
        let edits = parse_sed_edits(&parse(
            "sed --in-place=.bak --expression=s/a/b/ .git/config",
        ));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from(".git/config")]);
    }

    #[test]
    fn short_attached_expression_is_not_a_cluster() {
        // `-e's/a/b/'` would historically have been parsed by the
        // short-cluster decomposer, swallowing the script characters as
        // if they were boolean flags. The attached script must not
        // consume the following positional as the sed script.
        let edits = parse_sed_edits(&parse("sed -i -e's/a/b/' .git/config"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from(".git/config")]);
    }

    #[test]
    fn short_attached_script_file() {
        // `-fscript.sed` is the attached form of `-f script.sed`.
        let edits = parse_sed_edits(&parse("sed -i -fscript.sed target"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].files, vec![PathBuf::from("target")]);
    }
}
