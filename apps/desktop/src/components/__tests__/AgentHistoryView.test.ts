// @vitest-environment happy-dom
import { mount,flushPromises } from "@vue/test-utils";
import { expect,it,vi,beforeEach } from "vitest";
import AgentHistoryView from "../AgentHistoryView.vue";
import AgentStageSelector from "../AgentStageSelector.vue";
const {read}=vi.hoisted(()=>({read:vi.fn()}));
vi.mock("../../services/desktopServerClient",()=>({readAgentTerminalArchive:read}));
vi.mock("../../composables/renderTerminalArchive",()=>({renderTerminalArchive:async(s:{vt:string})=>s.vt}));
const archive=(task:string,id:string,code:number|null)=>({binding:{task_id:task,spawned_run_id:id},snapshot:{vt:id,cols:80,rows:24},observed_exit_code:code});
beforeEach(()=>read.mockReset());
it("focuses the read-only archive surface on request",async()=>{
  read.mockResolvedValue(null);const view=mount(AgentHistoryView,{props:{taskId:"task",attemptId:"a"},attachTo:document.body});await flushPromises();
  expect((view.vm as unknown as {focusContent:()=>boolean}).focusContent()).toBe(true);expect(document.activeElement).toBe(view.get("pre").element);view.unmount();
});
it.each([0,7,null])("shows recorded termination %s without inventing success",async(code)=>{
  read.mockResolvedValue(archive("task","a",code));const view=mount(AgentHistoryView,{props:{taskId:"task",attemptId:"a"}});await flushPromises();
  expect(view.text()).toContain(code===null?"Exit status unknown":`Exit ${code}`);expect(read).toHaveBeenCalledExactlyOnceWith("task","a");view.unmount();
});
it("renders remote captured scrollback and verifies its canonical task/run binding",async()=>{
  const load=vi.fn().mockResolvedValue(archive("owner-task","remote-run",7));
  const view=mount(AgentHistoryView,{props:{taskId:"owner-task",attemptId:"remote-run",sourceKey:"lan:owner:owner-task",loadArchive:load}});await flushPromises();
  expect(load).toHaveBeenCalledExactlyOnceWith("remote-run");expect(view.get("pre").text()).toBe("remote-run");expect(view.text()).toContain("Exit 7");
  await view.setProps({taskId:"different-owner",sourceKey:"lan:different:different-owner",loadArchive:vi.fn().mockResolvedValue(archive("owner-task","remote-run",7))});await flushPromises();
  expect(view.text()).toContain("Archive identity mismatch");expect(view.get("pre").text()).toBe("");view.unmount();
});
it("ignores stale attempt and task responses and exposes missing history",async()=>{
  let resolveA!:(v:unknown)=>void;read.mockImplementationOnce(()=>new Promise(r=>resolveA=r)).mockResolvedValueOnce(archive("other","b",7)).mockResolvedValueOnce(null);
  const view=mount(AgentHistoryView,{props:{taskId:"task",attemptId:"a"}});await view.setProps({taskId:"other",attemptId:"b"});await flushPromises();resolveA(archive("task","a",0));await flushPromises();
  expect(view.get("pre").text()).toBe("b");await view.setProps({attemptId:"missing"});await flushPromises();expect(view.text()).toContain("Historical output unavailable");expect(view.get("pre").text()).toBe("");view.unmount();
});
it("shows Latest first and historical attempts newest to oldest without changing identity or ordinals",async()=>{
  const attempts = [
    { id: "run-build-1", stage: "build", startedAt: "older", cwd: null, archived: true, recordedLaunch: true, observedExitCode: 0 },
    { id: "run-review-1", stage: "review", startedAt: "middle", cwd: null, archived: true, recordedLaunch: true, observedExitCode: 0 },
    { id: "run-build-2", stage: "build", startedAt: "newer", cwd: null, archived: true, recordedLaunch: true, observedExitCode: 0 },
  ];
  const view=mount(AgentStageSelector,{props:{selected:"",attempts}});
  expect(view.findAll("option").map(o=>o.attributes("value"))).toEqual(["","run-build-2","run-review-1","run-build-1"]);
  expect(view.findAll("option").slice(1).map(o=>o.text())).toEqual([
    "build · attempt 3 · newer",
    "review · attempt 2 · middle",
    "build · attempt 1 · older",
  ]);
  await view.get("select").setValue("run-build-2");await view.get("select").setValue("");expect(view.emitted("select")).toEqual([["run-build-2"],[""]]);view.unmount();
});
