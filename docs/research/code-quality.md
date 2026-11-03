# Code quality: what predicts trouble in code agents write

> 2026-11-03. Research for the Stats tab's code quality section; **built:** [code quality metrics](../specs/code-quality-metrics.md). Primary sources were read where linked; "abstract only" marks a claim checked only against an abstract. The numbers for this repo were measured with `kanna-quality` on main at 906a8d25.

Agents write most of kanna-v4. Test count and pass rate say nothing here, since every test must pass. The question is whether the code itself is getting worse as agents add to it. The literature points to four things to watch: how complexity concentrates in a few functions, how much code is copied or there only to pass calls on, which changes later needed fixing, and what the tests actually run. Each is cheap to measure per commit on main except coverage.

## Contents

1. [SlopCodeBench](#1-slopcodebench)
2. [God functions and concentration](#2-god-functions-and-concentration)
3. [Verbosity and duplication](#3-verbosity-and-duplication)
4. [Coverage](#4-coverage)
5. [Bug-introducing commits (SZZ)](#5-bug-introducing-commits-szz)
6. [Churn, coupling and hotspots](#6-churn-coupling-and-hotspots)
7. [Agent-written code](#7-agent-written-code)
8. [Rejected](#8-rejected)
9. [This repo, measured](#9-this-repo-measured)

## 1. SlopCodeBench

Orlanski et al., *SlopCodeBench: Benchmarking How Coding Agents Degrade Over Long-Horizon Iterative Tasks*, [arXiv:2603.24755](https://arxiv.org/abs/2603.24755) (v2, May 2026). Metrics engine: [`scb-check`](https://github.com/gabeorlanski/scb-check) 0.1.3, source read.

- **Structural erosion:** "the fraction of the codebase's total complexity mass that resides in high-complexity functions". mass(f) = CC(f) × √SLOC(f); erosion = Σ mass(f) over functions with CC > 10, divided by Σ mass(f) over all functions. The cutoff of 10 is radon's band edge. `scb-check` also reports the same share with cognitive complexity in place of CC.
- **Verbosity:** |flagged lines ∪ clone lines ∪ trivial-wrapper lines| / LOC.
  - **Flagged:** 137 AST-Grep rules (197 rule ids in 0.1.3), all Python idioms such as `for-range-len` and `redundant-bool-ternary`.
  - **Clone lines:** every line of every instance of a tree-sitter subtree (function, if, for, while, with, try, match) of at least 3 SLOC whose normalized form hashes the same as another's. Identifiers are renamed `$VAR1…` in order of appearance; literals become `$STR`, `$INT`, `$FLOAT`, `$BOOL` and `$NONE`; comments are dropped. These are Type-1 and Type-2 clones.
  - **Wrappers:** functions that only delegate.
- **Computed per checkpoint** of the agent's trajectory. Missing checkpoints are left out, not filled in.
- **Findings:**
  - Erosion rises in 77% of agent trajectories and verbosity in 75.5%.
  - Against 473 human repos, agent code is 2.3× more verbose and 2.0× more eroded.
  - An "anti-slop" prompt lowers where an agent starts but not how fast it degrades.
  - Erosion also rises in 53% of human repos.

**Here:** erosion ports as defined, with CC from tree-sitter-rust and tree-sitter-swift. Verbosity keeps the clone and wrapper parts, with subtree hashing ported to Rust and Swift node kinds. The flagged-rule part is left out, because the rules are Python's (§8).

## 2. God functions and concentration

- **Cyclomatic complexity** (McCabe 1976): decision points + 1. Radon counts each `if`, `elif`, loop, `except`, boolean operator and comprehension.
- **Cognitive complexity** ([Campbell, SonarSource white paper](https://www.sonarsource.com/docs/CognitiveComplexity.pdf), read):
  - +1 for each break in linear flow: `if`, ternary, `switch` (once for the whole switch), loops, `catch`, labelled jumps, each sequence of like boolean operators, and recursion.
  - Nesting adds +1 per level for `if`, ternary, `switch`, loops and `catch`. `else if` and `else` add +1 but no nesting increment.
  - Lambdas and nested functions raise the nesting level.
- **The size confound:** after controlling for size, class metrics lost their link to faults ([El Emam et al. 2001](https://doi.org/10.1109/32.935855), abstract only). Gil & Lalouche (2017) found a metric's validity largely follows its correlation with size. So complexity is reported as a share of the whole, or weighted by size as SlopCodeBench's mass is, never as a raw total.
- **Concentration:** code metrics are highly skewed, and their Gini coefficient stays stable as a project evolves, so a change in it is a signal (Vasa et al., ICSM 2009, abstract only). SlopCodeBench's harness reports the Gini of CC as `cc_concentration`.
- **Thresholds in use:**
  - PMD reports CC ≥ 10 per method and cognitive ≥ 15.
  - PMD's God Class rule is WMC ≥ 47, ATFD > 5 and TCC < ⅓ (Lanza & Marinescu 2006).
  - Clippy's `cognitive_complexity` (25) is restriction-only, and its own docs say it doesn't measure cognitive complexity.

## 3. Verbosity and duplication

- **Clone types** (Roy & Cordy 2007): I identical but for layout and comments; II also differing in identifiers and literals; III with statements changed or added; IV the same behaviour written differently.
- **Thresholds:** token-based detectors use minimums of 50 tokens (jscpd's default) or 100 (CPD's documented examples).
- **Clones and faults:** in Juergens et al. (ICSE 2009, read), 52% of clone groups had been changed inconsistently, and 3–23% of those inconsistencies were faults. Type-3 clones are more bug-prone than Type-1 and Type-2 (Mondal et al. 2015, abstract only).
- **AI-era duplication:** [GitClear 2025](https://www.gitclear.com/ai_assistant_code_quality_2025_research) found copy/pasted lines rose from 8.3% (2021) to 12.3% (2024), while moved (refactored) lines fell from 25% to under 10%. This is correlational.

## 4. Coverage

- **Coverage and effectiveness:** controlling for suite size, coverage correlates only weakly with suite effectiveness, and "should not be used as a quality target" ([Inozemtseva & Holmes, ICSE 2014](https://cs.ubc.ca/~rtholmes/papers/icse_2014_inozemtseva.pdf), read). At file level it shows no correlation with post-release bugs (Kochhar et al. 2017, abstract only).
- **Coverage at Google** ([Ivanković et al., FSE 2019](https://homes.cs.washington.edu/~rjust/publ/google_coverage_fse_2019.pdf), read):
  - Its most useful form is **delta coverage**, "the code coverage of only the lines that were modified or added in a changelist", shown in code review.
  - It sets voluntary levels instead of targets, for example project ≥ 75% and changelist ≥ 80%.
- **Tools here:**
  - Rust: `cargo llvm-cov`. Branch coverage needs nightly flags, which `RUSTC_BOOTSTRAP=1` allows on the pinned stable toolchain.
  - Swift: `swift test --enable-code-coverage`, exported with Xcode's `llvm-cov`.
  - The e2e scenarios: Rust binaries built with `-C instrument-coverage`, writing profiles in continuous mode (`%c`) so a stopped daemon keeps its counts.

## 5. Bug-introducing commits (SZZ)

- **The algorithm** ([Śliwerski, Zimmermann & Zeller, MSR 2005](https://thomas-zimmermann.com/publications/files/sliwerski-msr-2005.pdf), read): find fixes, take the lines each fix deleted or changed, and blame them on the fix's parent. The commits that last touched those lines are the candidates for having introduced the bug.
- **Precision is poor:**
  - Only about half of keyword-found "fixes" are fixes ([Herbold et al., EMSE 2022](https://arxiv.org/abs/1911.08938)).
  - On 76,046 Linux pairs, B-SZZ reached precision 0.42 and recall 0.58; R-SZZ, which keeps only the most recent candidate, reached 0.59 and 0.45, the best F1 ([Lyu et al., TSE 2024](https://arxiv.org/html/2308.05060v2)).
  - 17% of fixes only add lines, so blame has nothing to follow.
- **Recommendations:**
  - Blame with `-w`.
  - Drop blank, comment and test lines.
  - Prefer explicit links between fix and cause (Linux's `Fixes:` tags) to keyword matching.

## 6. Churn, coupling and hotspots

- **Process metrics:**
  - Relative churn predicts defect density (Nagappan & Ball, ICSE 2005).
  - Process metrics beat code metrics because code metrics barely change between releases (Moser et al. 2008; Rahman & Devanbu, ICSE 2013).
  - Change entropy outpredicts prior faults (Hassan, ICSE 2009, read).
- **Just-in-time prediction** (Kamei et al., TSE 2013, read): 14 features per change. The strongest were files touched, lines added relative to file size, and whether the change is a fix. Developer experience features mean little when one kind of agent writes everything.
- **Hotspots** (Tornhill): change frequency × complexity, so the complex files that keep changing come first.
- **Change coupling** correlates with defects (D'Ambros et al. 2009, abstract only).

## 7. Agent-written code

- **AIDev** (456K agent PRs, [arXiv 2507.15003](https://arxiv.org/abs/2507.15003)): agent PRs are accepted less often.
- **Cursor adopters** ([arXiv 2511.04427](https://arxiv.org/abs/2511.04427)): a difference-in-differences study of about 800 repos. Velocity rises briefly, while static-analysis warnings and complexity rise and stay up.
- **SlopCodeBench and GitClear** (§1, §3): agents degrade with each iteration, and duplication grows.

Taken together, duplication, the concentration of complexity and later fixes are the trends to watch, merge by merge.

## 8. Rejected

- **Test count and pass rate:** every test must pass, and a count is a size proxy.
- **A coverage target:** Goodhart's law. A target invites tests that run lines without checking them. Coverage shows as a trend and as the lines each landing left unrun.
- **The Maintainability Index:** mostly size, unexplained constants, and averages that hide hotspots ([van Deursen 2014](https://avandeursen.com/2014/08/29/think-twice-before-using-the-maintainability-index/)).
- **Lines of code as output:** agents inflate it. It shows only as context, the denominator of erosion and verbosity.
- **Halstead measures:** they track size and add nothing that a reader acts on.
- **SlopCodeBench's flagged lines:**
  - Its rules are Python idioms.
  - Rust's nearest equivalents are clippy's style lints, which need a compile per commit, while every other measure here reads git's objects without building.
  - A later step could count clippy warnings in the coverage run, which compiles anyway.
- **Token-window clone detection (jscpd style):** tried first, with 50-token windows. It reported 11% duplication, dominated by data tables such as the CLI's command list. SlopCodeBench's subtree hashing finds whole functions and blocks, matching the benchmark the numbers are compared against.
- **Plain B-SZZ and keyword-only fix detection:**
  - On this repo's history, matching words like "lost" and "nothing" in landing subjects marked feature merges as fixes, and blamed up to 34 landings each.
  - Fixes are found by a `fix-` branch, strong words in the subject's first clause, a task's own description, or a `Fixes:` trailer.
  - Only the most recent candidate is kept, as R-SZZ does.
- **Change coupling and JIT defect models:** worth adding once the simpler trends have run for a while. Experience features don't apply to agents.
- **Clippy's `cognitive_complexity`:** its own docs disown the name. Kanna computes cognitive complexity from the white paper's rules.

## 9. This repo, measured

On main at 906a8d25: 255 Rust and Swift files outside tests, 63,541 code lines and 5,005 functions. Reading and parsing them from git's objects takes 0.8 s, and all 556 first-parent commits take 13.6 s.

| Measure | 2026-10-08 (82ddb8c7) | 2026-11-03 |
|---|---|---|
| Code lines | 5,308 | 63,541 |
| Erosion | 37.8% | 48.5% |
| Verbosity | 3.4% | 5.3% |
| Functions over CC 10 | 15 | 230 |
| Pass-through wrappers | 11 | 63 |
| Gini of cognitive complexity | | 0.73 |

- **Concentration:** the top 5% of functions hold 41% of the cognitive complexity.
- **Worst function:** the daemon's `run` (crates/kanna-daemon/src/main.rs), with CC 154, cognitive 216 and 677 lines.
- **Trend:** erosion has risen with nearly every week of agent work, as SlopCodeBench predicts.
- **SZZ:** commit subjects alone find 14 fix landings, 9 of them traced back to a landing, with a median of 69 hours from landing to fix.
