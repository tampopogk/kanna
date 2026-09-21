/**
 * Fuzzy file-path matcher inspired by VSCode's fuzzyScorer.
 *
 * Matches query characters in order (not necessarily contiguous) against a
 * file path, producing a numeric score and the indices of matched characters.
 *
 * Scoring bonuses (per character):
 *   +1  base match
 *   +1  exact case
 *   +8  start of string
 *   +5  after path separator (/)
 *   +4  after word separator (_ - . space)
 *   +2  camelCase boundary
 *   +5  first 3 consecutive matches (each), +3 thereafter
 *   -1  per target character skipped inside the matched span
 *
 * Those bonuses are a sum, so on their own they grow with how many word
 * boundaries a target happens to offer: a query scattered across a long
 * filename out-earns the same query matching a short filename exactly. The
 * skip penalty above and the match-quality tiers in `scoreSingle` keep the
 * score a statement about the match rather than about the target's length.
 */

export interface FuzzyResult {
  score: number;
  indices: number[];
}

const WORD_SEPARATORS = new Set(["_", "-", ".", " "]);

const SKIPPED_CHAR_PENALTY = 1;

/**
 * Cross-candidate ranking tiers for a filename match.
 *
 * Per-character bonuses answer "how well did this query align inside this one
 * target"; they cannot answer "which of these targets did the user mean",
 * because a longer filename can collect more of them. Tiering the *kind* of
 * filename match settles that first, so typing a filename always lists that
 * file above the partial matches it appears inside, and the character score
 * only orders candidates of equal kind.
 */
const FILENAME_TIER_SCATTERED = 1000;
const FILENAME_TIER_SUBSTRING = 3000;
const FILENAME_TIER_PREFIX = 5000;
const FILENAME_TIER_EXACT = 7000;

/**
 * Sub-point tiebreak among filename matches of equal quality: the name with
 * the least left over is the one that was typed. Kept strictly below one point
 * so it can only order candidates the character score left tied, never invert
 * a real scoring difference.
 */
function leftoverPenalty(matched: number, filenameLength: number): number {
  const leftover = Math.max(filenameLength - matched, 0);
  return leftover / (leftover + 1);
}

function filenameTier(query: string, filename: string): number {
  const q = query.toLowerCase();
  const name = filename.toLowerCase();
  if (q === name) return FILENAME_TIER_EXACT;
  if (name.startsWith(q)) return FILENAME_TIER_PREFIX;
  if (name.includes(q)) return FILENAME_TIER_SUBSTRING;
  return FILENAME_TIER_SCATTERED;
}

function isUpperCase(ch: string): boolean {
  return ch !== ch.toLowerCase() && ch === ch.toUpperCase();
}

function charScore(
  target: string,
  targetIdx: number,
  queryChar: string,
  consecutive: number,
): { score: number; consecutive: number } {
  let s = 1; // base match

  // exact case bonus
  if (target[targetIdx] === queryChar) {
    s += 1;
  }

  // positional bonuses
  if (targetIdx === 0) {
    s += 8; // start of string
  } else {
    const prev = target[targetIdx - 1];
    if (prev === "/") {
      s += 5; // after path separator
    } else if (WORD_SEPARATORS.has(prev)) {
      s += 4; // after word separator
    } else if (isUpperCase(target[targetIdx]) && !isUpperCase(prev)) {
      s += 2; // camelCase boundary
    }
  }

  // consecutive bonus with plateau
  const nextConsecutive = consecutive + 1;
  if (nextConsecutive > 1) {
    s += nextConsecutive <= 3 ? 5 : 3;
  }

  return { score: s, consecutive: nextConsecutive };
}

/**
 * Score a single query against a target string using a greedy forward scan
 * with a preference for word-boundary matches.
 *
 * Returns null if the query doesn't match.
 */
function scoreSegment(
  query: string,
  target: string,
): FuzzyResult | null {
  const queryLower = query.toLowerCase();
  const targetLower = target.toLowerCase();

  if (queryLower.length > targetLower.length) return null;

  // Quick rejection: every query char must exist somewhere
  for (let i = 0; i < queryLower.length; i++) {
    if (targetLower.indexOf(queryLower[i], 0) === -1) return null;
  }

  // Two-pass approach:
  // 1) Prefer word-boundary aligned matches (greedy)
  // 2) Fall back to first-available match
  // Take the higher score.
  const boundaryResult = scorePath(query, queryLower, target, targetLower, true);
  const greedyResult = scorePath(query, queryLower, target, targetLower, false);

  if (!boundaryResult && !greedyResult) return null;
  if (!boundaryResult) return greedyResult;
  if (!greedyResult) return boundaryResult;
  return boundaryResult.score >= greedyResult.score ? boundaryResult : greedyResult;
}

function isBoundary(target: string, idx: number): boolean {
  if (idx === 0) return true;
  const prev = target[idx - 1];
  if (prev === "/" || WORD_SEPARATORS.has(prev)) return true;
  if (isUpperCase(target[idx]) && !isUpperCase(prev)) return true;
  return false;
}

function scorePath(
  query: string,
  queryLower: string,
  target: string,
  targetLower: string,
  preferBoundary: boolean,
): FuzzyResult | null {
  const indices: number[] = [];
  let totalScore = 0;
  let consecutive = 0;
  let targetIdx = 0;
  let skipped = 0;

  const take = (idx: number, qi: number) => {
    // Characters stepped over inside the matched span are what makes a match
    // sparse; charging for them is what stops a long target from out-scoring a
    // tight one purely because it offered more boundaries to land on.
    if (indices.length > 0) skipped += idx - indices[indices.length - 1] - 1;
    if (idx > targetIdx) consecutive = 0;
    const cs = charScore(target, idx, query[qi], consecutive);
    totalScore += cs.score;
    consecutive = cs.consecutive;
    indices.push(idx);
    targetIdx = idx + 1;
  };

  for (let qi = 0; qi < queryLower.length; qi++) {
    const qch = queryLower[qi];
    let matched = false;

    if (preferBoundary) {
      // Look for a boundary match first (scan ahead)
      let boundaryIdx = -1;
      for (let ti = targetIdx; ti < targetLower.length; ti++) {
        if (targetLower[ti] === qch && isBoundary(target, ti)) {
          boundaryIdx = ti;
          break;
        }
      }
      if (boundaryIdx !== -1) {
        take(boundaryIdx, qi);
        matched = true;
      }
    }

    if (!matched) {
      // First-available match
      for (let ti = targetIdx; ti < targetLower.length; ti++) {
        if (targetLower[ti] === qch) {
          take(ti, qi);
          matched = true;
          break;
        }
      }
    }

    if (!matched) return null;
  }

  return { score: totalScore - skipped * SKIPPED_CHAR_PENALTY, indices };
}

/**
 * Fuzzy-match a query against a file path.
 *
 * Supports multi-part queries: "comp btn" matches both "comp" and "btn"
 * against the path independently, requiring all parts to match.
 *
 * Applies a filename bonus: if the query (or any part) matches entirely within
 * the filename portion, the score is boosted.
 */
export function fuzzyMatch(query: string, filePath: string): FuzzyResult | null {
  const trimmed = query.trim();
  if (!trimmed) return null;

  const parts = trimmed.split(/\s+/);

  // Single query — score against filename first, then full path
  if (parts.length === 1) {
    return scoreSingle(parts[0], filePath);
  }

  // Multi-part: all parts must match, aggregate scores
  let totalScore = 0;
  const allIndices: number[] = [];

  for (const part of parts) {
    const result = scoreSingle(part, filePath);
    if (!result) return null;
    totalScore += result.score;
    allIndices.push(...result.indices);
  }

  return { score: totalScore, indices: [...new Set(allIndices)].sort((a, b) => a - b) };
}

function scoreSingle(query: string, filePath: string): FuzzyResult | null {
  const lastSlash = filePath.lastIndexOf("/");
  const filename = lastSlash >= 0 ? filePath.slice(lastSlash + 1) : filePath;

  // Try filename first — boost if matched
  const filenameResult = scoreSegment(query, filename);
  if (filenameResult) {
    const offset = lastSlash >= 0 ? lastSlash + 1 : 0;
    return {
      // Filename matches sort above directory matches, exact above prefix
      // above substring above scattered, shortest name first within a tier.
      score: filenameResult.score
        + filenameTier(query, filename)
        - leftoverPenalty(filenameResult.indices.length, filename.length),
      indices: filenameResult.indices.map((i) => i + offset),
    };
  }

  // Fall back to full path
  return scoreSegment(query, filePath);
}
