import { fork, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:http";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { expect, it, vi } from "vitest";

it("packaged extension reconnects HTTP and reconciles after process replacement without native resend", async () => {
  const tempRoot=fileURLToPath(new URL("../../../../.tmp/",import.meta.url));
  await mkdir(tempRoot,{recursive:true});const directory=await mkdtemp(join(tempRoot,"copilot-wake-wire-"));
  const resources=new URL("../../../../crates/kanna-server/resources/copilot-wake/",import.meta.url);
  const source=await readFile(new URL("extension.mjs",resources),"utf8");
  // Only resolve the host's SDK import to a scripted public API fixture. The
  // packaged transport and bridge bytes are otherwise unchanged; no CLI/model.
  await writeFile(join(directory,"extension.mjs"),source.replace("'@github/copilot-sdk/extension'","'./sdk-fixture.mjs'"));
  await writeFile(join(directory,"bridge.mjs"),await readFile(new URL("bridge.mjs",resources)));
  await writeFile(join(directory,"sdk-fixture.mjs"),`
    export async function joinSession() { process.stdin.resume(); return {
      sessionId: process.env.SESSION_ID,
      on() { return () => {}; },
      async send(options) { process.send({call:'send',options}); return 'native-queue'; },
      async getEvents() { return JSON.parse(process.env.FIXTURE_HISTORY || '[]'); }
    }; }
  `);
  let connections=0; let registrationRequests=0;
  const receipts:any[]=[];const sends:any[]=[];const errors:string[]=[];const children:ChildProcess[]=[];
  const message="[Kanna supervisor] pending synthetic batch [Kanna wake task/run/native/watch-1]";
  const server=createServer(async(request,response)=>{
    if(request.method==="GET") {
      if (++registrationRequests === 1) { response.writeHead(503); response.end(); return; }
      connections++;
      const binding={taskId:"task",runId:"run",sessionId:"native",connectionId:`epoch-${connections}`};
      response.writeHead(200,{"Content-Type":"text/event-stream"});
      response.write(`data: ${JSON.stringify({type:"registered",protocol:1,binding})}\n\n`);
      const command=JSON.stringify({type:connections===1?"send":"inspect",attempt:{id:"watch-1",binding,message}});
      // Deliberately split a frame across writes, exercising real stream framing.
      response.write(`data: ${command.slice(0,31)}`);response.write(`${command.slice(31)}\n\n`);
    } else {
      let text="";for await(const chunk of request)text+=chunk;
      receipts.push(JSON.parse(text));
      response.writeHead(receipts.length===1?500:200,{"Content-Type":"application/json"});
      response.end(JSON.stringify({recorded:receipts.length>1,acknowledged:false}));
    }
  });
  server.listen(0,"127.0.0.1");await once(server,"listening");
  const address=server.address() as {port:number};
  const start=(history:any[])=>{
    const child=fork(join(directory,"extension.mjs"),[],{execArgv:[],stdio:["pipe","pipe","pipe","ipc"],env:{
      KANNA_TASK_ID:"task",KANNA_STAGE_RUN_ID:"run",SESSION_ID:"native",
      KANNA_SERVER_BASE_URL:`http://127.0.0.1:${address.port}`,FIXTURE_HISTORY:JSON.stringify(history),
    }});
    child.on("message",m=>sends.push(m));child.stderr?.on("data",d=>errors.push(d.toString()));children.push(child);return child;
  };
  const stop=async(child:ChildProcess)=>{
    if(child.exitCode!==null||child.signalCode!==null)return;
    const exited=once(child,"exit");child.kill("SIGKILL");await exited;
  };
  try {
    const first=start([]);
    await vi.waitFor(()=>expect(receipts.some(r=>r.connectionId==="epoch-2"&&r.kind==="accepted")).toBe(true),{timeout:5_000});
    expect(sends).toEqual([{call:"send",options:{prompt:message,mode:"enqueue"}}]);
    expect(receipts.find(r=>r.connectionId==="epoch-2"&&r.kind==="accepted")).toEqual({runId:"run",sessionId:"native",connectionId:"epoch-2",attemptId:"watch-1",kind:"accepted",messageId:"native-queue"});
    expect(errors.join("")).toContain("mailbox retained");
    await stop(first);
    start([{id:"history-event",type:"user.message",data:{content:message}}]);
    await vi.waitFor(()=>expect(receipts.some(r=>r.connectionId==="epoch-3"&&r.kind==="observed")).toBe(true),{timeout:5_000});
    expect(receipts.find(r=>r.connectionId==="epoch-3"&&r.kind==="observed")).toEqual({runId:"run",sessionId:"native",connectionId:"epoch-3",attemptId:"watch-1",kind:"observed",eventId:"history-event",content:message});
    expect(sends).toHaveLength(1);
    expect(receipts.every(r=>r.source===undefined&&r.acknowledgeBatchId===undefined)).toBe(true);
    // Abrupt loss of the provider-owned stdin pipe stops reconnecting too.
    const last=children.at(-1)!;
    last.stdin!.end();
    await vi.waitFor(()=>expect(last.exitCode).toBe(0),{timeout:3_000});
  } finally {
    for(const child of children)await stop(child);
    server.closeAllConnections();await new Promise<void>(resolve=>server.close(()=>resolve()));
    await rm(directory,{recursive:true,force:true});
  }
},15_000);
