import type { IBufferRange, Terminal } from "@xterm/xterm"

export interface TerminalLogicalLine {
  text: string
  startRow: number
  endRow: number
}

/**
 * Reconstruct the logical line containing a zero-based buffer row. Xterm
 * exposes soft wrapping on the continuation row, so both directions are
 * needed when a provider is asked about the middle of a wrapped path.
 */
export function readTerminalLogicalLine(
  term: Terminal,
  bufferRow: number,
): TerminalLogicalLine | null {
  const buffer = term.buffer.active
  if (!buffer.getLine(bufferRow)) return null

  let startRow = bufferRow
  while (startRow > 0 && buffer.getLine(startRow)?.isWrapped) startRow -= 1

  let endRow = bufferRow
  while (endRow + 1 < buffer.length && buffer.getLine(endRow + 1)?.isWrapped) endRow += 1

  const parts: string[] = []
  for (let row = startRow; row <= endRow; row += 1) {
    const line = buffer.getLine(row)
    if (!line) return null
    // This matches xterm's web-link provider. Mapping through cells below
    // corrects offsets when an early wrap involves a wide character.
    parts.push(line.translateToString(true))
  }
  return { text: parts.join(""), startRow, endRow }
}

/** Map a JavaScript string offset in a logical line back to xterm cells. */
function mapStringOffset(
  term: Terminal,
  lineIndex: number,
  columnIndex: number,
  stringOffset: number,
): [number, number] | null {
  const buffer = term.buffer.active
  const cell = buffer.getNullCell()
  let startColumn = columnIndex

  while (stringOffset > 0) {
    const line = buffer.getLine(lineIndex)
    if (!line) return null
    for (let column = startColumn; column < line.length; column += 1) {
      line.getCell(column, cell)
      const chars = cell.getChars()
      if (cell.getWidth()) {
        stringOffset -= chars.length || 1

        // Xterm can leave an empty final cell when a wide character wraps.
        // Its continuation occupies the first cell of the following row.
        if (column === line.length - 1 && chars === "") {
          const nextLine = buffer.getLine(lineIndex + 1)
          if (nextLine?.isWrapped) {
            nextLine.getCell(0, cell)
            if (cell.getWidth() === 2) stringOffset += 1
          }
        }
      }
      if (stringOffset < 0) return [lineIndex, column]
    }
    lineIndex += 1
    startColumn = 0
  }
  return [lineIndex, startColumn]
}

export function terminalRangeForLogicalMatch(
  term: Terminal,
  logicalLine: TerminalLogicalLine,
  start: number,
  length: number,
): IBufferRange | null {
  const startPosition = mapStringOffset(term, logicalLine.startRow, 0, start)
  if (!startPosition) return null
  const endPosition = mapStringOffset(term, startPosition[0], startPosition[1], length)
  if (!endPosition) return null
  return {
    start: { x: startPosition[1] + 1, y: startPosition[0] + 1 },
    end: { x: endPosition[1], y: endPosition[0] + 1 },
  }
}
