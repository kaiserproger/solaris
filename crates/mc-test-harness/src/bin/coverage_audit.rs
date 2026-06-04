use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

const DEFAULT_LEDGER: &str = "docs/VALIDATION_LEDGER.md";

#[derive(Debug, Clone, PartialEq, Eq)]
struct LedgerRow {
    id: String,
    row: String,
    scope: String,
    status: String,
    evidence: String,
    gaps: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditResult {
    denominator: usize,
    numerator: usize,
    rows: Vec<RowAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RowAudit {
    id: String,
    row: String,
    status: String,
    counts: bool,
    reason: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("coverage-audit: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let ledger_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LEDGER));
    if let Some(extra) = args.next() {
        return Err(format!(
            "unexpected argument `{extra}`; usage: coverage-audit [ledger.md]"
        ));
    }

    let ledger = fs::read_to_string(&ledger_path).map_err(|error| {
        format!(
            "failed to read validation ledger `{}`: {error}",
            ledger_path.display()
        )
    })?;
    let result = audit_ledger(&ledger)?;

    println!("# Validation Coverage Audit");
    println!();
    println!("Ledger: `{}`", ledger_path.display());
    println!("Denominator: {} in-scope rows", result.denominator);
    println!("Numerator: {} conservative ready rows", result.numerator);
    println!(
        "Coverage: {:.2}%",
        coverage_percent(result.numerator, result.denominator)
    );
    println!();
    println!("| ID | Status | Counts | Reason |");
    println!("|---|---|---|---|");
    for row in result.rows {
        println!(
            "| {} | `{}` | {} | {} |",
            row.id,
            row.status,
            if row.counts { "yes" } else { "no" },
            row.reason
        );
    }

    Ok(())
}

fn audit_ledger(markdown: &str) -> Result<AuditResult, String> {
    let rows = parse_frozen_denominator(markdown)?;
    let mut audited = Vec::with_capacity(rows.len());
    let mut numerator = 0;

    for row in rows {
        let (counts, reason) = count_reason(&row);
        if counts {
            numerator += 1;
        }
        audited.push(RowAudit {
            id: row.id,
            row: row.row,
            status: row.status,
            counts,
            reason,
        });
    }

    Ok(AuditResult {
        denominator: audited.len(),
        numerator,
        rows: audited,
    })
}

fn parse_frozen_denominator(markdown: &str) -> Result<Vec<LedgerRow>, String> {
    let mut in_table = false;
    let mut rows = Vec::new();

    for (line_index, line) in markdown.lines().enumerate() {
        if line.trim() == "## Frozen M100 Denominator" {
            in_table = true;
            continue;
        }

        if in_table && line.starts_with("## ") {
            break;
        }

        if !in_table || !line.starts_with('|') {
            continue;
        }

        let columns = split_markdown_row(line);
        if columns[0] == "ID" || columns[0].starts_with("---") {
            continue;
        }
        if columns.len() != 7 {
            return Err(format!(
                "malformed frozen denominator row at line {}: expected 7 columns, found {}; remove internal `|` characters from the table row",
                line_index + 1,
                columns.len()
            ));
        }
        if columns[2] != "In scope" {
            continue;
        }

        rows.push(LedgerRow {
            id: columns[0].to_owned(),
            row: columns[1].to_owned(),
            scope: columns[2].to_owned(),
            status: unquote_status(columns[3]),
            evidence: columns[4].to_owned(),
            gaps: columns[5].to_owned(),
        });
    }

    if !in_table {
        return Err("missing `## Frozen M100 Denominator` section".to_owned());
    }
    if rows.is_empty() {
        return Err("frozen denominator table has no in-scope rows".to_owned());
    }

    Ok(rows)
}

fn split_markdown_row(line: &str) -> Vec<&str> {
    line.trim_matches('|').split('|').map(str::trim).collect()
}

fn unquote_status(status: &str) -> String {
    status.trim_matches('`').to_owned()
}

fn count_reason(row: &LedgerRow) -> (bool, String) {
    if row.status != "ready" {
        return (false, format!("status `{}` is not `ready`", row.status));
    }

    let evidence = row.evidence.to_ascii_lowercase();
    let gaps = row.gaps.to_ascii_lowercase();
    if has_disqualifying_evidence(&evidence) || has_disqualifying_evidence(&gaps) {
        return (
            false,
            "mentions unit-only, negated, Solaris-only, missing, blocked, or manual-pending evidence"
                .to_owned(),
        );
    }

    if !has_runtime_evidence(&evidence) {
        return (false, "no focused runtime test evidence found".to_owned());
    }

    if !has_vanilla_or_client_evidence(&evidence) {
        return (
            false,
            "no vanilla oracle or real-client evidence found".to_owned(),
        );
    }

    (
        true,
        "ready with runtime plus vanilla/client evidence".to_owned(),
    )
}

fn has_runtime_evidence(evidence: &str) -> bool {
    ["harness", "runtime", "test", "wire-probe", "prismlauncher"]
        .iter()
        .any(|needle| evidence.contains(needle))
}

fn has_vanilla_or_client_evidence(evidence: &str) -> bool {
    [
        "real-client",
        "real client",
        "prismlauncher",
        "vanilla oracle",
        "vanilla-oracle",
        "vanilla server oracle",
        "vanilla capture",
        "vanilla-capture",
    ]
    .iter()
    .any(|needle| evidence.contains(needle))
}

fn has_disqualifying_evidence(text: &str) -> bool {
    [
        "lacks oracle",
        "lacks client",
        "lacks real-client",
        "lacks real client",
        "lacks vanilla",
        "unit-only",
        "unit tests only",
        "wire-probe-only",
        "wire-probe capture",
        "solaris-only",
        "solaris harness capture",
        "manual-pending",
        "not run",
        "missing",
        "blocked",
        "lacks required",
        "no linked",
        "no linked oracle",
        "no linked client",
        "no linked vanilla",
        "no oracle",
        "no client evidence",
        "no real-client",
        "no real client",
        "no vanilla",
        "without oracle",
        "without client evidence",
        "without real-client",
        "without real client",
        "without vanilla",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn coverage_percent(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        (numerator as f64 / denominator as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_rows_do_not_count_even_with_harness_words() {
        let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| B1 | Block edits | In scope | `partial` | Harness and vanilla capture exist. | More cases missing. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

        let result = audit_ledger(markdown).expect("audit should parse");

        assert_eq!(result.denominator, 1);
        assert_eq!(result.numerator, 0);
        assert_eq!(result.rows[0].reason, "status `partial` is not `ready`");
    }

    #[test]
    fn ready_rows_require_runtime_and_vanilla_or_client_evidence() {
        let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| B1 | Block edits | In scope | `ready` | Runtime harness plus vanilla oracle capture. | None for normal path. | M95 |
| B2 | Doors | In scope | `ready` | Unit tests only. | No client evidence. | M95 |
| B3 | Fluids | In scope | `ready` | Runtime harness. | No oracle evidence. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

        let result = audit_ledger(markdown).expect("audit should parse");

        assert_eq!(result.denominator, 3);
        assert_eq!(result.numerator, 1);
        assert!(result.rows[0].counts);
        assert!(!result.rows[1].counts);
        assert!(!result.rows[2].counts);
    }

    #[test]
    fn current_ledger_has_no_conservative_ready_rows() {
        let ledger = include_str!("../../../../docs/VALIDATION_LEDGER.md");

        let result = audit_ledger(ledger).expect("ledger should parse");

        assert_eq!(result.denominator, 46);
        assert_eq!(result.numerator, 0);
    }

    #[test]
    fn ready_rows_do_not_count_negated_or_unit_only_evidence() {
        let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| B1 | Block edits | In scope | `ready` | Runtime harness and unit tests only. | No oracle evidence. | M95 |
| B2 | Doors | In scope | `ready` | Runtime harness without vanilla evidence. | None for normal path. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

        let result = audit_ledger(markdown).expect("audit should parse");

        assert_eq!(result.denominator, 2);
        assert_eq!(result.numerator, 0);
        assert!(!result.rows[0].counts);
        assert!(!result.rows[1].counts);
    }

    #[test]
    fn wire_probe_only_does_not_satisfy_both_evidence_legs() {
        let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| C1 | Chunk streaming | In scope | `ready` | Long wire-probe runtime test. | None for normal path. | M95 |
| C2 | Lighting payloads | In scope | `ready` | Wire-probe runtime test plus vanilla capture. | None for normal path. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

        let result = audit_ledger(markdown).expect("audit should parse");

        assert_eq!(result.denominator, 2);
        assert_eq!(result.numerator, 1);
        assert!(!result.rows[0].counts);
        assert!(result.rows[1].counts);
    }

    #[test]
    fn negated_or_unlinked_oracle_client_phrases_fail_closed() {
        let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| B1 | Block edits | In scope | `ready` | Runtime harness lacks oracle evidence. | None for normal path. | M95 |
| B2 | Doors | In scope | `ready` | Runtime harness has no linked vanilla oracle evidence. | None for normal path. | M95 |
| B3 | Fluids | In scope | `ready` | Runtime harness has no vanilla evidence. | None for normal path. | M95 |
| B4 | Movement | In scope | `ready` | Runtime harness has no oracle evidence. | None for normal path. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

        let result = audit_ledger(markdown).expect("audit should parse");

        assert_eq!(result.denominator, 4);
        assert_eq!(result.numerator, 0);
        assert!(result.rows.iter().all(|row| !row.counts));
    }

    #[test]
    fn bare_or_non_vanilla_captures_do_not_count_as_oracle_or_client_evidence() {
        let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| C1 | Chunk streaming | In scope | `ready` | Runtime harness plus capture. | None for normal path. | M95 |
| C2 | Lighting payloads | In scope | `ready` | Runtime harness plus wire-probe capture. | None for normal path. | M95 |
| C3 | Collision | In scope | `ready` | Runtime harness plus Solaris harness capture. | None for normal path. | M95 |
| C4 | Falling blocks | In scope | `ready` | Runtime harness plus vanilla capture. | None for normal path. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

        let result = audit_ledger(markdown).expect("audit should parse");

        assert_eq!(result.denominator, 4);
        assert_eq!(result.numerator, 1);
        assert!(!result.rows[0].counts);
        assert!(!result.rows[1].counts);
        assert!(!result.rows[2].counts);
        assert!(result.rows[3].counts);
    }

    #[test]
    fn malformed_denominator_rows_fail_loudly() {
        let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| B1 | Block edits | In scope | `ready` | Runtime harness | vanilla capture. | None. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

        let error = audit_ledger(markdown).expect_err("row should be malformed");

        assert!(error.contains("malformed frozen denominator row at line 6"));
        assert!(error.contains("expected 7 columns, found 8"));
    }
}
