import { afterEach, describe, expect, it, vi } from "vitest";
import { createWakeBridge } from "../../../../crates/kanna-server/resources/copilot-wake/bridge.mjs";

const binding = { taskId:"task", runId:"run", sessionId:"native", connectionId:"first" };
const attempt = { id:"watch-1", binding, message:"[Kanna supervisor] synthetic pending batch [Kanna wake task/run/native/watch-1]" };
const cleanups: (() => void)[] = [];
afterEach(() => { for (const close of cleanups.splice(0)) close(); vi.useRealTimers(); });
function fixture() {
  let listener = (_:any) => {};
  const events:any[] = [];
  const session = {
    sessionId:"native",
    on: vi.fn((fn: (event:any) => void) => { listener=fn; return () => { listener=()=>{}; }; }),
    send:vi.fn(async (_:unknown) => "queue-1"),
    getEvents:vi.fn(async () => events),
  };
  const report = vi.fn(async (_:unknown) => {});
  const bridge=createWakeBridge(session,{taskId:"task",runId:"run"},report);
  cleanups.push(()=>bridge.dispose());
  return {bridge,session,report,events,emit:(event:any)=>listener(event)};
}
const tick = async () => { for(let i=0;i<10;i++) await Promise.resolve(); };

describe("bundled Copilot wake bridge", () => {
  it("sends labelled native enqueue once; duplicate transport frames never resubmit",async()=>{
    const f=fixture();
    await f.bridge.handle({type:"registered",protocol:1,binding});
    await f.bridge.handle({type:"send",attempt});await tick();
    await f.bridge.handle({type:"send",attempt});await tick();
    expect(f.session.send).toHaveBeenCalledExactlyOnceWith({prompt:attempt.message,mode:"enqueue"});
    expect(f.report).toHaveBeenCalledWith({...binding,attemptId:attempt.id,kind:"accepted",messageId:"queue-1"});
    expect(f.report.mock.calls.every(([r]:any)=>r.acknowledgeBatchId===undefined && r.source===undefined)).toBe(true);
  });
  it("reconciles a lost report using retained queue acceptance on a new connection",async()=>{
    const f=fixture();f.report.mockRejectedValueOnce(Error("connection lost"));
    await f.bridge.handle({type:"registered",protocol:1,binding});
    await f.bridge.handle({type:"send",attempt});await tick();
    const replacement={...binding,connectionId:"replacement"};
    await f.bridge.handle({type:"registered",protocol:1,binding:replacement});
    await f.bridge.handle({type:"inspect",attempt:{...attempt,binding:replacement}});
    expect(f.session.send).toHaveBeenCalledTimes(1);
    expect(f.report).toHaveBeenLastCalledWith({...replacement,attemptId:attempt.id,kind:"accepted",messageId:"queue-1"});
  });
  it("fresh extension state never resends uncertain input; a later native event resolves absent history",async()=>{
    const f=fixture();await f.bridge.handle({type:"registered",protocol:1,binding});
    await f.bridge.handle({type:"inspect",attempt});
    expect(f.report).toHaveBeenLastCalledWith(expect.objectContaining({kind:"uncertain"}));
    const event={id:"different-history-id",type:"user.message",data:{content:attempt.message}};
    f.events.push(event);f.emit(event);await tick();
    expect(f.report).toHaveBeenLastCalledWith({...binding,attemptId:attempt.id,kind:"observed",eventId:event.id,content:attempt.message});
    expect(f.session.send).not.toHaveBeenCalled();
  });
  it("keeps ambiguous history uncertain and refuses changed text or foreign sessions",async()=>{
    const f=fixture();await f.bridge.handle({type:"registered",protocol:1,binding});
    f.events.push(...["a","b"].map(id=>({id,type:"user.message",data:{content:attempt.message}})));
    await f.bridge.handle({type:"inspect",attempt});
    expect(f.report).toHaveBeenLastCalledWith(expect.objectContaining({kind:"uncertain",error:"ambiguous native history; retained without resend"}));
    await expect(f.bridge.handle({type:"send",attempt:{...attempt,message:"[Kanna supervisor] changed"}})).rejects.toThrow("text changed");
    await expect(f.bridge.handle({type:"registered",protocol:1,binding:{...binding,sessionId:"foreign"}})).rejects.toThrow("mismatch");
    expect(f.session.send).not.toHaveBeenCalled();
  });
  it("a hanging native send does not block recovery, and disposal releases its deadline",async()=>{
    vi.useFakeTimers();const f=fixture();f.session.send.mockImplementation(()=>new Promise<string>(()=>{}));
    await f.bridge.handle({type:"registered",protocol:1,binding});
    await f.bridge.handle({type:"send",attempt});await tick();
    await f.bridge.handle({type:"inspect",attempt});
    await vi.advanceTimersByTimeAsync(10_000);
    expect(f.report).toHaveBeenLastCalledWith(expect.objectContaining({kind:"uncertain",error:"native queue receipt timed out"}));
    expect(f.session.send).toHaveBeenCalledTimes(1);
    f.bridge.dispose();expect(vi.getTimerCount()).toBe(0);
  });
});
