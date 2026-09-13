import { resolve } from "node:path";
import { it } from "vitest";
import { WebDriverClient } from "../helpers/webdriver";
import { taskReferenceScenario } from "../helpers/taskReferenceScenario";
it("preserves task references alongside the real PTY across switches, resizing and restart", async () => {
  await taskReferenceScenario(new WebDriverClient(), resolve("../.."));
});
