import type { TaskDesignSummary } from "../lib/api/types";
/** "Design · Prototype · 2 open" for the task header's way into the design. */
export function designButtonLabel(design: TaskDesignSummary): string {
  const position = design.positions.find((candidate) => candidate.name === design.position)?.label ?? design.position;
  const parts = [design.inDesignStage ? `Design · ${position}` : "Design · handed off"];
  if (design.openThreads) parts.push(`${design.openThreads} open`);
  if (design.uncertainFeedback) parts.push(`${design.uncertainFeedback} not delivered`);
  return `${parts.join(" · ")} ›`;
}
