# Code quality: what predicts trouble in code agents write

> 2026-11-03, corrected 2026-11-04 against `scb-check`'s source and output. Research for the Stats tab's code quality section; **built:** [code quality metrics](../specs/code-quality-metrics.md). Primary sources were read where linked; "abstract only" marks a claim checked only against an abstract. The numbers for this repo were measured with `kanna-quality` on main at 3966e489.

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
  - **Flagged:** 137 AST-Grep rules (197 rule ids in 0.1.3) in eight files: conditionals, defensive code, loops and comprehensions, dict patterns, type conversions, booleans, annotations and misc. They are written against Python's grammar, but most describe shapes any language has: an `else` after a `return`, an `if` nested alone in an `if`, a loop that only appends, a function with three `try` blocks, an exception caught only to be re-raised. On human code they are verbosity's largest part (below).
  - **Clone lines:** every line of every instance of a tree-sitter subtree (function, if, for, while, with, try, match) of at least 3 SLOC whose normalized form hashes the same as another's. Identifiers are renamed `$VAR1…` in order of appearance; literals become `$STR`, `$INT`, `$FLOAT`, `$BOOL` and `$NONE`; comments are dropped. These are Type-1 and Type-2 clones.
  - **Wrappers** (`trivial-wrapper`): a function whose only statement, docstrings aside, is a `return` of anything, plus module-level aliases of a function. It isn't limited to forwarding the function's own parameters.
  - Every part counts code lines only: lines where a token starts, by Python's tokenizer, minus docstrings. A multi-line string counts once.
- **Computed per checkpoint** of the agent's trajectory. Missing checkpoints are left out, not filled in.
- **Findings:**
  - Erosion rises in 77% of agent trajectories and verbosity in 75.5%.
  - Against 473 human repos, agent code is 2.3× more verbose and 2.0× more eroded.
  - An "anti-slop" prompt lowers where an agent starts but not how fast it degrades.
  - Erosion also rises in 53% of human repos.

**`scb-check` on human code** (0.1.3 run on five Python packages, read as human-written):

| Package | Verbosity | Flagged | Clones | Wrappers | Erosion |
|---|---|---|---|---|---|
| `email` (stdlib 3.13) | 0.267 | 0.120 | 0.107 | 0.053 | 0.572 |
| `http` (stdlib 3.13) | 0.200 | 0.133 | 0.051 | 0.026 | 0.597 |
| `click` 8.5 | 0.236 | 0.141 | 0.066 | 0.050 | 0.472 |
| `rich` 15 | 0.217 | 0.188 | 0.015 | 0.018 | 0.592 |
| `typer` 0.24 | 0.313 | 0.123 | 0.066 | 0.157 | 0.620 |

*(The parts overlap, so they don't add up to verbosity.)* These sit near the paper's human mean for verbosity (0.19 ± 0.11) and above it for erosion (0.34 ± 0.22), whose 473 repos include much smaller ones.

**Here:** each part ports as defined, with tree-sitter-rust and tree-sitter-swift. The port also parses Python, and on these five packages it gives `scb-check`'s erosion, cognitive erosion, function counts, clone lines and wrapper counts exactly or within a few lines ([the comparison](../specs/code-quality-metrics.md#matching-slopcodebench)). The flagged part runs 36 Rust and Swift rules, each a port of one or more of SlopCodeBench's rules to the same shape ([the rules](../specs/code-quality-metrics.md#the-rules)). That covers the rules that flag the most lines in the packages above, but not all 197, so verbosity here reads somewhat low against the benchmark's.

**What the first port got wrong:** it reported 0.053 verbosity and 0.485 erosion for this repo, both optimistic, for five reasons:
1. It left the flagged part out, taking the rules for Python idioms.
2. It counted as wrappers only functions passing exactly their parameters on, 63 instead of 1,258.
3. It gave a closure passed to `.map` or `.filter` nothing in CC, where SlopCodeBench counts every comprehension.
4. It counted lines holding only `}`, which Python doesn't have. That inflated the denominator and let two-line blocks pass as three-line clones.
5. It hashed leaves by their text, not their kind.

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
- **Clippy for the flagged lines:** many of the ported rules are clippy lints too (`collapsible_if`, `needless_return`, `redundant_closure`, `len_zero`, `question_mark`), but clippy needs a compile per commit, while every other measure here reads git's objects without building. It also flags things SlopCodeBench doesn't, and doesn't cover Swift. The rules run on the tree-sitter tree already parsed.
- **ast-grep itself for the rules:** `ast-grep-core` links its own tree-sitter, a different version from the one `kanna-quality` uses, and the two can't share a parsed tree. Rules written against the tree in Rust parse each file once.
- **Token-window clone detection (jscpd style):** tried first, with 50-token windows. It reported 11% duplication, dominated by data tables such as the CLI's command list. SlopCodeBench's subtree hashing finds whole functions and blocks, matching the benchmark the numbers are compared against.
- **Plain B-SZZ and keyword-only fix detection:**
  - On this repo's history, matching words like "lost" and "nothing" in landing subjects marked feature merges as fixes, and blamed up to 34 landings each.
  - Fixes are found by a `fix-` branch, strong words in the subject's first clause, a task's own description, or a `Fixes:` trailer.
  - Only the most recent candidate is kept, as R-SZZ does.
- **Change coupling and JIT defect models:** worth adding once the simpler trends have run for a while. Experience features don't apply to agents.
- **Clippy's `cognitive_complexity`:** its own docs disown the name. Kanna computes cognitive complexity from the white paper's rules.

## 9. This repo, measured

On main at 3966e489: 277 Rust and Swift files outside tests, 54,900 code lines and 5,289 functions. Reading and parsing them from git's objects, rules included, takes 1.2 s.

| Measure | 2026-10-08 (82ddb8c7) | 2026-11-04 (3966e489) |
|---|---|---|
| Code lines | 4,210 | 54,900 |
| Erosion | 48.5% | 61.1% |
| Verbosity | 9.6% | 11.3% |
| Functions over CC 10 | 25 (5.5%) | 469 (8.9%) |
| Trivial wrappers | 107 | 1,275 |
| Gini of cognitive complexity | 0.71 | 0.73 |

- **Against the benchmark:** erosion (0.61) is near the paper's agent mean (0.68 ± 0.20) and well above its human mean (0.34 ± 0.22), though level with the mature Python packages above (0.47–0.62). Verbosity (0.11) is below both the agent mean (0.44 ± 0.18) and those packages (0.20–0.31), partly because the rule set is a partial port.
- **Where verbosity comes from:** 2,885 lines in trivial wrappers, 2,381 flagged lines (function-wide `error-soup` and `formatted-error-heavy` are half of them) and 970 clone lines.
- **Worst function:** the daemon's `run` (crates/kanna-daemon/src/main.rs), with CC 232, cognitive 216 and 560 lines.
- **Trend:** erosion has risen with nearly every week of agent work, as SlopCodeBench predicts.
- **SZZ:** commit subjects alone find 14 fix landings, 9 of them traced back to a landing, with a median of 69 hours from landing to fix.
