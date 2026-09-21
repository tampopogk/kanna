# Kanna's workflow pattern language

## Status

First draft, 2026-09-19. This is a design document to argue with. Nothing here
is implemented: no workflow JSON, no agent definition, no code. The patterns
below describe how work *should* be recognised and routed; Kanna's current
workflows are mapped against them honestly in "Where Kanna actually is", and
the disagreements are left as disagreements.

## Why a pattern language

Christopher Alexander's *A Pattern Language* has 253 patterns, from the scale of
a region down to the scale of a windowsill. Each one names a recurring situation,
the forces pulling on it, and what an experienced builder does about it. The Gang
of Four took his form and applied it one level down — patterns for the *inside*
of a program, for how objects are arranged once you already know what you are
building.

This document takes the form and applies it somewhere else: **patterns for what
arrives at the door of a software factory, and what an experienced person does
about each kind.** Not how to structure the code. How to recognise the request.

The owner's framing, which is the entire point of the exercise:

> It's kind of like an experience matching tool. You might have knowledge, you
> might have skills. But if you lack experience, you don't know how to recognise
> patterns of problems and then apply patterns of solutions.

An agent has knowledge and skills. What it does not have is the thing a senior
engineer has after fifteen years: the instinct that says *this is a sales-driven
feature, it will be built regardless, write down who overrode the objection and
move on*. That instinct is not reasoning ability. It is pattern recognition
built from having been burned. A pattern language is how you hand it over
without the fifteen years.

Alexander's patterns reference each other by name — that is what makes a
language rather than a catalogue. The same rule applies here: every pattern
below names the patterns it resolves into and the patterns it escalates to, and
a real request is almost always two patterns composed, not one.

## The two axes

There are exactly two questions to ask about incoming work.

### Origin — should this happen, and who is accountable for saying so

- **Internal.** You think it should be better. You found a bug. Engineering
  believes the architecture is wrong. The proposer and the beneficiary are the
  same organisation, often the same person.
- **External.** A user hit a bug. A user wants a feature. A user has a pain that
  may or may not be yours to solve. Something outside the company — a platform
  vendor, a certificate expiry, an OS release — changed under you.
- **Organisational.** Another team needs something. Product wants something.
  Sales already sold something.

### Uncertainty — how it gets done

- **U0 — mechanical.** You know the change and you know the place.
- **U1 — known outcome, unknown shape.** You can describe what "done" looks like
  to somebody who does not work here, but you cannot yet name the files.
- **U2 — known problem, unknown outcome.** You can describe what is wrong. You
  cannot yet describe what "fixed" is, and reasonable people would choose
  differently.
- **U3 — an idea, and no problem yet.** Somebody had a thought. It might be
  worth something. There is no stated pain behind it.

### Size is an output, not a question

You cannot know whether a request is a rename or a rewrite until uncertainty is
resolved. Asking for size at intake is the estimate-before-research mistake, and
it is the single most common way a factory commits to a number it invented. Size
falls out of the work; it is never an input to choosing the pattern.

There is exactly one place below where size appears, and it appears as a
*trigger* rather than an estimate: when the work of resolving U2 comes back and
says "two years", that answer is itself the signal to switch patterns. That is
not asking for size up front. That is reacting to size once it is known.

### What each axis actually selects

This is the central claim of the whole document, and the thing most worth
arguing with:

> **Uncertainty selects the shape of the work. Origin selects the accountability
> artifact and the acceptance test.**

Uncertainty decides whether there is a research stage, a plan gate, a review, or
just a commit. Origin decides who signs, what gets written down so the decision
survives, and who is allowed to say "done".

That is why the sales-driven feature's answer is not a workflow. Its uncertainty
might be U0 — the change may be completely obvious. What makes it dangerous is
entirely its origin, so what it needs is a record, not a stage.

Nearly every real request is therefore **one origin pattern composed with one
uncertainty pattern**. "A customer paid for a thing that spans four subsystems"
is `The Sales-Driven Feature` + `The Shaped Delivery`. "We need to move off the
deprecated API before their cutoff and we know exactly where it is" is
`The Forced Move` + `The Mechanical Change`.

### The grid

|                    | U0 mechanical | U1 outcome, no shape | U2 problem, no outcome | U3 idea, no problem |
|--------------------|---------------|----------------------|------------------------|---------------------|
| **Internal**       | Mechanical Change | Shaped Delivery | Open Problem → possibly Engineering Rewrite | Exploratory Idea |
| **External**       | Reported Fault, Forced Move | Reported Fault + Shaped Delivery | Open Problem | rare |
| **Organisational** | Cross-Team Request + Mechanical Change | Sales-Driven Feature or Cross-Team Request, + Shaped Delivery | Cross-Team Request + Open Problem | rare, and worth asking why it exists |

Ten patterns follow. Five are uncertainty patterns, four are origin patterns,
and one is an artifact that several of the others produce.

## How to read a pattern

Each pattern has:

- **Name** — what to call it out loud, so two people can disagree about which
  one this is.
- **Context** — one sentence, written to be *matched against a real request*.
  This is the most important line in the pattern, because it is what a task
  manager (or a person) matches on. If the context sentence needs the request
  to already be classified, it is a bad context sentence.
- **Forces** — what is pulling in opposite directions. If nothing is in tension,
  it is not a pattern, it is a procedure.
- **Response** — what an experienced person does. Opened with *Therefore*, after
  Alexander.
- **Failure mode** — the specific bad outcome this pattern exists to prevent.
  A pattern with no failure mode is decoration.
- **Resolves into** — the smaller patterns inside it, or the artifact it
  produces.
- **Escalation** — what has to be observed for this pattern to become a more
  expensive one, and which *direction* that move runs. A pattern needing
  something ahead of where the task is grows the same task; one needing
  something behind it becomes a new task from the branch. See "Escalation over
  selection".

---

# The uncertainty patterns

## 1. The Mechanical Change

**Context.** *Use this when you already know both the change and the place it
goes, and could describe the diff before opening the editor.*

Origin: any. Uncertainty: U0.

**Forces.** The change is small and the cost of being wrong about it is small.
But the process that exists to protect large changes does not know that, and
will apply itself anyway. Meanwhile the one thing that makes a mechanical change
*not* mechanical — the place turning out not to be the place — is invisible until
somebody starts.

**Therefore.** Do it. One stage, commit, done. No plan, no panel, no design
discussion. The reviewer of a mechanical change is the test suite. Ceremony
applied to a rename costs more than the rename and teaches everyone that the
ceremony is noise.

**Failure mode.** Ceremony. A three-line fix that acquires a plan stage, a
specialty review panel, and two days of latency, after which the organisation
concludes that the process is the problem and stops using it for the changes
that needed it.

**Resolves into.** Nothing. A commit. This is a terminal pattern.

**Escalation.** One trigger: **the place was not the place.** The moment the
change cannot be made where you expected — the abstraction does not exist, the
call site has four callers you did not know about, the test that should have
covered it does not — stop and escalate to `The Shaped Delivery`. If what is
missing is only scrutiny, that is a review stage ahead of where the task is and
the task grows. If what is missing is a decided shape, that belongs behind it,
so the correction is a new task from this branch. Either way this is not a
failure of the initial classification; entering here and being wrong is cheaper
than entering higher and being right.

---

## 2. The Reported Fault

**Context.** *Use this when someone says the system did the wrong thing, and you
have their account of it but not yet a reproduction.*

Origin: external (a user hit it) or internal (you found it). Uncertainty:
claimed U0, actual unknown until reproduced.

**Forces.** A bug report feels mechanical — someone has already told you what is
wrong, so surely you just fix it. But a report is a description of a *symptom*
from outside the system, and the distance between a symptom and a fault is the
whole job. Pushing against that: a reported fault has a person waiting, and
diagnosis looks like inaction to them.

**Therefore.** Treat the reproduction as the deliverable of the first round, not
as a preliminary. Until it reproduces you do not know the uncertainty level, so
you cannot know the pattern. Once it reproduces, the fault's location tells you
which pattern you are actually in, and that is usually `The Mechanical Change`
and occasionally `The Shaped Delivery`.

**Origin matters here more than anywhere else.** With an *external* report, the
reporter is the acceptance test — the fix is not done when the test passes, it is
done when the person who reported it agrees, and somebody has to go tell them.
With an *internal* one, nobody is waiting, which means the honest question is
whether it is worth fixing at all; an internally found bug with no user behind it
is a candidate for `The Decision Record` saying you chose not to.

**Failure mode.** Two of them. Fixing the symptom that was described rather than
the fault that produced it — the report goes quiet and the fault stays. Or
fixing a bug the user never had, because their description was reconstructed
from memory and the reproduction was never demanded.

**Resolves into.** `The Mechanical Change` once located. `The Shaped Delivery`
when the fault is structural. `The Decision Record` when the answer is "working
as intended" or "not worth it", which is a real answer that must be written
down rather than left as silence.

**Escalation.** Escalate when the reproduction does not come, or when it
reproduces somewhere other than where it was reported. A fault that cannot be
reproduced after honest effort is not a fault yet — it is `The Open Problem`,
and pretending otherwise produces speculative fixes that are indistinguishable
from noise. Research belongs behind a fix rather than ahead of it, so that is a
new task from this branch, not a stage bolted onto this one.

---

## 3. The Shaped Delivery

**Context.** *Use this when you can describe what "done" looks like to somebody
who does not work here, but you cannot yet name the files you will touch.*

Origin: any. Uncertainty: U1.

**Forces.** The outcome is agreed, so there is pressure to start immediately —
the thinking feels finished. But the shape is not known, and a build that
discovers halfway through that the approach is wrong has spent its budget on the
wrong thing and now argues to keep it. Against that: a plan for work whose shape
is genuinely obvious is a tax.

**Therefore.** Separate deciding the shape from executing it, and put a human
gate between them. Plan, gate, build, review. The plan is cheap to be wrong
about and cheap to throw away; the build is neither. The gate exists precisely
because the plan is the last moment where changing your mind is free.

The review of a shaped delivery must be able to send findings to *either* side
of the gate: an implementation defect goes back to the build, but a finding that
says the approach cannot produce a correct result however well it is executed
goes back to the plan. Sending a structural finding to the builder produces a
better-executed wrong thing.

**Failure mode.** Estimating before research: a number is given, the number
becomes a commitment, and the shape is then chosen to fit the number rather than
the problem. The second failure mode is the plan that was never a gate — it was
written, approved without reading, and diverged from silently.

**Resolves into.** A plan artifact, then one or more `The Mechanical Change`
inside the build. If the plan comes back saying the outcome itself is unclear,
it resolves into `The Open Problem` — which sits behind this one, so that is a
new task from this branch rather than a stage added to this one.

**Escalation.** Escalate to `The Open Problem` when the plan stage cannot
converge on one shape because reasonable people would choose different outcomes
— that is not a planning failure, it is a misclassification: the outcome was
never actually agreed, and a misclassification is corrected by starting the
right task from this branch rather than by growing this one backwards. Escalate
to `The Engineering Rewrite` when the plan comes back and the answer is measured
in quarters.

**De-escalation.** A plan that concludes "this is one function in one file"
should drop to `The Mechanical Change` and skip its own review panel. This
direction is rarer and much safer than escalation, and it is the one the
existing replacement mechanism already permits outright: a stage the task has
not entered may simply be removed.

---

## 4. The Open Problem

**Context.** *Use this when you can describe what is wrong but not what "fixed"
looks like, and two competent people would pick different answers.*

Origin: any — most often external (a user described a pain, not a feature) or
internal (engineering believes something is structurally wrong). Uncertainty: U2.

**Forces.** A stated pain arrives with a proposed solution attached, because
people describe problems in the language of the fix they imagined. Implementing
the proposal is fast, satisfying, and frequently solves nothing. Against that:
research has no visible output, is easy to extend indefinitely, and is
indistinguishable from stalling if it is not bounded.

**Therefore.** Research first, and *only* research: alternatives, evidence,
tradeoffs, assumptions, and the questions that remain. Record a recommendation
and stop at a human gate. A recommendation is not authorisation. The person
accountable for the origin — the owner for an internal doubt, the owner on
behalf of the user for an external pain — chooses the outcome, and only then
does the work become a `Shaped Delivery`.

Critically: **"the answer is not a code change" is a legitimate output.** A
setting, a document, a better error message, or a decision to do nothing are all
valid conclusions, and a research stage whose only allowed output is a feature
is not research.

**Failure mode.** Building the user's suggested solution literally. They asked
for a button; the button ships; the pain is unchanged; the feature is now
permanent surface area you maintain forever and the underlying problem is
harder to see because something addresses it.

**Resolves into.** `The Shaped Delivery` when an outcome is chosen. `The
Decision Record` when the outcome is "not ours to solve" or "not now". `The
Engineering Rewrite` when the honest answer is that the architecture is the
problem.

**Escalation.** This pattern is usually *escalated into* rather than out of. It
is reached from `The Shaped Delivery` when the outcome turns out not to be
agreed, and from `The Reported Fault` when nothing reproduces. Both arrivals are
new tasks cut from the branch of the task that discovered the
misclassification, because research belongs behind the work, not ahead of it. Escalating out of
it, upward, means only one thing: `The Engineering Rewrite`, and that transition
needs the scrutiny described there.

---

## 5. The Exploratory Idea

**Context.** *Use this when somebody had an idea and there is no stated problem
behind it yet.*

Origin: usually internal. Uncertainty: U3.

**Forces.** Ideas with no problem behind them are where the genuinely new things
come from, so a factory that refuses them only ever does incremental work.
But an idea has no acceptance test, which means nothing can ever declare it
finished or failed — and the longer it runs, the more expensive it becomes to
say it was nothing, because somebody's three weeks are in it.

**Therefore.** Timebox it, build the cheapest thing that produces a verdict, and
**write the exit condition down before starting, including the one that says this
was a dead end.** A dead end recorded in writing is a *success* of this pattern:
it is the output you paid for. The explicit statement matters because sunk cost
does not announce itself — it arrives disguised as "we're so close" and as an
unwillingness to waste what has been spent.

The exit is recorded rather than merely taken, for a specific reason: an
unrecorded dead end returns anonymously in six months, proposed by somebody who
was not there, and gets explored again at full price.

**Failure mode.** Sunk cost. The idea acquires a roadmap slot because effort was
spent on it, not because a problem was found. The second, quieter failure mode:
the exploration succeeds, finds something real, and is then shipped directly —
skipping the step where somebody checks whether a user has this problem.

**Resolves into.** Either a problem statement, which is now `The Open Problem`
or `The Shaped Delivery` and re-enters the front door properly, or `The Decision
Record` saying this was explored, here is what was learned, and here is why it
stopped.

**Escalation.** There is no escalation out of this pattern that keeps it as an
exploration. If the idea finds a real problem, it *exits* to a pattern with an
accountable origin. An exploration that grows stages without ever acquiring a
problem statement is the failure mode, not an escalation.

---

# The origin patterns

## 6. The Sales-Driven Feature

**Context.** *Use this when the work arrives already decided, with money attached
and a customer name on it, and engineering's technical objection is not a veto.*

Origin: organisational. Uncertainty: whatever it happens to be — often low.

**Forces.** A customer offering real money for something that makes no product
sense is a genuine business opportunity and a genuine product liability at the
same time. Engineering can see the liability and cannot see the balance sheet.
The company can see the balance sheet and cannot see the maintenance cost.
Neither side has the whole picture, and the decision belongs to the side with
the money.

**Therefore — and this is the important part — the response is not a workflow.**
Build it. That is a legitimate business call and engineering does not hold a
veto over it. What the pattern requires is a `The Decision Record` written
*before* the work starts, naming:

- the **objection** — what engineering believes will go wrong, in specific and
  falsifiable terms;
- **who overrode it** — a person, not a department;
- the **date**;
- the **prediction** — what we expect to observe if the objection was right, and
  roughly when;
- the **revisit trigger** — the customer churning, the second customer asking
  for the opposite, the maintenance cost crossing a line.

The record exists so that the post-mortem in eight months is a *lookup* rather
than archaeology. Without it, the conversation eight months later is six people
reconstructing who wanted this and why, none of them agreeing, and the real
lesson — that this class of deal produces this class of cost — never gets
learned, so it is bought again.

**Failure mode.** Two, symmetrical. Engineering treats the objection as a veto,
slow-walks the work, and destroys its credibility as a partner for the next
decision. Or the objection is simply swallowed, nothing is written down, and the
organisation carries the cost permanently without ever connecting it to the
choice that caused it.

**Resolves into.** `The Decision Record` first, then whichever uncertainty
pattern the work actually needs — usually `The Shaped Delivery`, sometimes
`The Mechanical Change`.

**Escalation.** The uncertainty half escalates normally. The origin half does not
escalate; it is already at the top. What it has instead is a *revisit*: the
prediction in the record either came true or it did not, and somebody should
look. A record nobody ever reads back is the same as no record.

---

## 7. The Cross-Team Request

**Context.** *Use this when another team needs something from you, their deadline
is real, and it is not your deadline.*

Origin: organisational. Uncertainty: any.

**Forces.** Their need is legitimate and their timeline is real. Your
architecture has opinions about what should exist, and the fastest thing to give
them is almost always a shim that violates those opinions. The shim will be
called temporary by both parties, sincerely, and will outlive everyone involved.

**Therefore.** Make the contract explicit and dated before building: what they
get, what you will *not* support, who the consumer is by name, and when it is
revisited. The named consumer is the whole trick — an interface with a named
consumer can be changed by talking to that consumer; an interface with an
unknown consumer can never be changed at all.

**Failure mode.** The permanent temporary surface. Discovered years later,
during an unrelated change, when it turns out three things depend on it, nobody
remembers agreeing to support it, and the team that asked for it no longer
exists.

**Resolves into.** `The Decision Record` naming the consumer and the expiry,
then `The Shaped Delivery` or `The Mechanical Change` for the work. If it is not
clear what they actually need — as opposed to what they asked for — it resolves
first into `The Open Problem`, with their engineer in the room.

**Escalation.** Escalate when the second consumer appears. One named consumer is
a favour; two consumers is a supported interface, and it should be re-entered as
`The Shaped Delivery` with the design attention an interface deserves. The
arrival of a second consumer is a concrete, observable trigger, which is what
makes it a usable escalation rule rather than a judgement call.

---

## 8. The Forced Move

**Context.** *Use this when something outside the company changed, no user asked
for anything, and the deadline is not negotiable.*

Origin: external, but with no user behind it — a platform vendor's deprecation,
an OS release, a certificate expiry, a dependency that stopped being maintained.
Uncertainty: usually U0 or U1.

**Forces.** There is no product value in this work whatsoever, which makes it
feel like waste, which makes it attractive to bundle other work into it — "we're
in there anyway". But the deadline is externally imposed and immovable, and
anything bundled into the work inherits that deadline without inheriting its
justification.

**Therefore.** Do the smallest thing that satisfies the external constraint, and
refuse the bundle. Keep the change boring and separable. If the forced move
exposes something that genuinely should be fixed, that is a *separate* item with
its own origin and its own pattern, and it does not get to ride the immovable
deadline.

**Failure mode.** The forced move becomes the vehicle for a rewrite. The
non-negotiable deadline now applies to work that was never scoped, and the team
is doing an architecture project under a cutoff date set by somebody else.

**Resolves into.** `The Mechanical Change` almost always. `The Shaped Delivery`
when the external change has no local equivalent and a shape has to be chosen.
Anything it exposes resolves into a *new* item, not into this one.

**Escalation.** Escalate only when the smallest compliant change is genuinely
impossible — the replacement API has no equivalent for something you rely on.
That is `The Open Problem` under a deadline — and, being a backwards move, a new
task from this branch. It is the worst quadrant to be in, and the reason forced
moves should be started early rather than at the cutoff.

---

## 9. The Engineering Rewrite

**Context.** *Use this when engineers conclude the architecture is wrong and
propose replacing it, and the people proposing it are the people who would do
it.*

Origin: internal. Uncertainty: U2 or U3 dressed as U1. Size: this is the one
pattern where size is part of the trigger — not because it was estimated up
front, but because once the shape resolves into quarters or years, the
commitment length itself changes which pattern applies.

**Forces.** The engineers are frequently *right* about the architecture. They
have the most information about it and they live with the cost daily. That is
exactly what makes this dangerous: the claim is made by the only people
qualified to evaluate it, and they are also the people who would spend two years
executing it. Nobody outside engineering can assess the technical premise, so
nobody outside engineering can say no, so it never receives the scrutiny that a
two-year *external* commitment — a customer contract, a market entry — would
receive automatically.

This is a structural problem, not a character problem. Honest, competent people
produce it reliably.

**Therefore.** Supply the external forcing function that the structure does not
provide:

1. **Name a user-visible outcome** the rewrite buys, and a measurement somebody
   outside engineering can read without trusting engineering's interpretation.
   "The code will be cleaner" is not one. "This class of incident stops
   happening, and here is the incident count" is.
2. **Get the premise verified by someone who will not do the work.** Independent
   of the proposers, with the authority to return "the premise is wrong".
3. **Demand a strangler path with shippable increments**, each of which is
   defensible on its own if the project stops after it. If no increment is
   individually justifiable, the project is a single two-year bet and should be
   evaluated as one.
4. **State a kill criterion in advance**, before anyone is invested — what
   observation would mean this was the wrong call, and who is allowed to call it.

If the proposal cannot survive those four, it is not ready, and that is a useful
result rather than an insult.

**Failure mode.** Two years, no shipped intermediate, no user-visible outcome,
and nobody in a position to stop it — because stopping it requires an outsider to
overrule engineering on a technical question, which never happens. The project
ends by attrition rather than by decision, and no one learns anything
transferable.

**Resolves into.** `The Decision Record` — the objection here runs the other
direction, recording what was promised and the prediction to check. Then an
ordered chain of `The Shaped Delivery`, each independently justifiable, with the
kill criterion checked between them.

**Escalation.** This pattern is almost always *arrived at* rather than chosen,
and it arrives disguised. The signal is repetition: the third time the same area
resists a small fix, the work was never `The Mechanical Change`. Recognising
that early — as `The Open Problem` about that subsystem — is much cheaper than
recognising it after a two-year commitment has been made. **Escalating into this
pattern should be hard on purpose.** It is the only pattern here where the
correct bias is against entry.

---

# The artifact pattern

## 10. The Decision Record

**Context.** *Use this when a decision is being made that somebody will want to
understand in a year, and the people who made it will not be reachable, will not
agree, or will not remember.*

Origin: any. Uncertainty: none — this is not work, it is a record of a choice.

**Forces.** Writing it down costs ten minutes now and pays off, unpredictably,
somewhere between never and years from now. Everything about the moment argues
against it: the decision feels obvious to everyone present, the reasoning feels
like it will be remembered, and the people who will need it are not in the room
to ask for it.

**Therefore.** Write down, at minimum: what was decided, what was decided
*against* and why, who decided, the date, and — the field most often missing and
most valuable — **the prediction**: what we expect to observe if this was the
wrong call, and roughly when we would see it. A decision record without a
prediction cannot be graded, and a record that cannot be graded teaches nothing.

The record is immutable. A later reversal is a *new* record that references the
old one. Editing the old record to match what happened destroys the only thing
it was for.

**Failure mode.** Archaeology. Eight months later, six people reconstruct a
decision from commit messages, half-remembered meetings, and a Slack thread that
has aged out of retention — and reach three different conclusions, none of which
is what happened. The cost is not the lost hour; it is that the same class of
decision is made again the same way, because nobody could establish what the
last one cost.

**Resolves into.** Nothing — it is a leaf. It is *produced by*
`The Sales-Driven Feature`, `The Cross-Team Request`, `The Engineering Rewrite`,
the "not now" exit of `The Open Problem`, the dead-end exit of
`The Exploratory Idea`, and the "working as intended" exit of
`The Reported Fault`.

**Escalation.** None. The only variable is whether it gets written, and the only
useful rule is that the patterns above treat it as mandatory rather than
encouraged.

---

# Where Kanna actually is

## The current workflows are one dial, not a set of patterns

Kanna's four public product-work workflows are:

| Workflow | Stages | What distinguishes it |
|---|---|---|
| `no-review` | implement (commit post) → pr (approve post) | no review stage |
| `single-reviewer` | implement → review → pr | one `review` agent |
| `specialized-reviewers` | implement → `qa-dispatcher` fan-out → pr | a dispatched specialty panel |
| `plan-build-review` | plan (manual gate) → build → review → pr | a plan stage in front |

Three of those four differ **only in how much review happens**. That is a dial
with three positions, not three patterns. It answers a question — how much
scrutiny does this deserve — that the axes above treat as an *output* of
uncertainty, and it answers it at task creation, which is the moment of least
information.

**Kanna's model currently encodes no information about origin at all.** Every
workflow starts at `in progress` or `plan`, which is to say every workflow
assumes the decision to do the work has already been made and is not worth
recording. For internal mechanical work that assumption is correct. For anything
arriving from sales, another team, or a two-year engineering conviction, it is
the whole problem.

## Mapping each existing workflow onto the axes

- **`no-review`** *is* `The Mechanical Change`, essentially exactly. It is also
  the fallback when a repo names no workflow, which — given "escalation over
  selection" below — is the right default rather than a gap. Verdict: **fits.**

- **`single-reviewer`** is the cheap end of `The Shaped Delivery`: U1 work where
  the shape is close enough to obvious that a plan gate would be ceremony, but a
  second pair of eyes is worth having. Verdict: **fits**, though it is better
  understood as a position on the escalation ladder than as a pattern of its own.

- **`specialized-reviewers`** is **not a pattern.** It is `single-reviewer` with
  a wider review fan. It differs from its neighbour in one dimension that the
  axes say is an output. The owner's own observation is the evidence: the
  dispatcher fans out to every specialty reviewer in cases where he would not.
  A dial position chosen at creation, by a party that systematically
  overestimates difficulty, is exactly the failure this document is arguing
  against. Verdict: **a dial, and a candidate for deletion** once escalation
  exists — see the open questions.

- **`plan-build-review`** is `The Shaped Delivery` proper, and it is the best
  existing fit in the repository. Its review stage already chooses its revision
  target *by the nature of the findings* — implementation defects return to
  `in progress`, structurally wrong approaches return to `plan`. That is the
  closest thing Kanna has today to movement between levels inside a running
  task, and it is the seed the escalation model should grow from. Verdict:
  **fits, and is the proof the idea works.**

- **`research`** (public, one manual stage, `researcher` agent) is `The Open
  Problem`, and it already has the two properties that pattern needs: it
  explores *what* outcome and *why* without planning delivery, and it parks at a
  human gate where a recommendation is explicitly not authorisation. When the
  owner chooses an outcome, the task manager **grows the same task** by appending
  a manual `plan` stage rather than starting a replacement, which is precisely
  "resolves into `The Shaped Delivery`" implemented. Verdict: **fits, and is the
  strongest existing evidence for this whole document.**

- **`architect-research`** (internal, one manual stage, `architect` agent) is
  *part* of what `The Engineering Rewrite` needs: a bounded verdict from someone
  who will not do the work, with `STOP-and-escalate` as a first-class outcome.
  But it is invoked by a task manager, which is not a party outside engineering,
  and it assesses one durable work item rather than a multi-quarter commitment.
  Verdict: **partial fit** — the right mechanism aimed at a smaller target.

- **`pr-review` / `pr-review-single` / `specialty-review` / `repository-setup`**
  are not product-work patterns and are out of scope here.

## Where a pattern is missing

Honestly, in order of how much it matters:

1. **`The Decision Record` has no home in Kanna.** Nothing in the system records
   a product decision, its objection, its author, and its prediction. The
   nearest relative is not a substitute: `human_review_decision` records
   authority to queue one PR, not a record of a choice and what it predicted.
   This is the single largest gap, and it is the response to the pattern the
   owner named first.

2. **`The Exploratory Idea` has no workflow.** `research` is the closest, but
   the `researcher` agent is pointed at product outcomes and returns a
   recommendation. Nothing in Kanna gives a dead end the status of a success, or
   requires an exit condition to be written before the exploration starts.

3. **The origin patterns have no representation whatsoever.** `The Sales-Driven
   Feature`, `The Cross-Team Request`, and `The Forced Move` all reduce, in
   today's Kanna, to "somebody writes a task prompt". Whether that is a gap or
   correct minimalism is an open question below — origin may belong in the
   prompt and nowhere else.

4. **`The Reported Fault`'s reproduction-first rule is not expressed anywhere.**
   A bug task today enters `in progress` and starts fixing. Whether that needs a
   workflow or just a line in the `implement` agent is, again, open.

Deliberately *not* missing: there is no gap for `The Engineering Rewrite` as a
workflow, and there should not be one. A two-year commitment is not a task.

---

# Escalation over selection

## The argument

Selection happens at task creation — the moment when the least is known about
the work. Every argument for removing size from the axes applies with equal
force to choosing review depth, plan gates, and research stages up front: they
are all estimates made before the research that would inform them.

And the selector is biased. Models overestimate difficulty; the qa-dispatcher
fans out to every specialty where the owner would fan out to none. A system that
asks an agent to pick the right workflow at creation is asking for a systematic
over-selection tax on every task, forever.

**Therefore: enter every pattern at the cheapest plausible level, and escalate
when a named trigger is observed.** Being wrong downward costs one escalation.
Being wrong upward costs the ceremony on every task that did not need it, and it
is invisible, because nobody ever sees the plan stage that was not necessary.

The escalation triggers in each pattern above are deliberately written as
*observations*, not judgements: the place was not the place; the reproduction
did not come; the plan could not converge; the second consumer appeared; the
third small fix in the same area failed. A trigger you can observe is one an
agent can act on without re-estimating.

## The ladder

```
The Mechanical Change          no-review
        ↓  the place was not the place, and only scrutiny is missing
        ↓  GROW — a review stage sits ahead of where the task is
The Shaped Delivery (cheap)    single-reviewer
        ↓  the shape has to be decided before it is built
        ↓  NEW TASK — a plan stage belongs behind where the task is
The Shaped Delivery (gated)    plan-build-review
        ↓  the outcome itself is not agreed
        ↓  NEW TASK — research belongs behind the work
The Open Problem               research
        ↓  the honest answer is measured in quarters
        ↓  NEITHER — this is not a task
The Engineering Rewrite        a decision record and a chain of deliveries
```

Running the other way, once an outcome is chosen, there is one routine forward
move and it grows the same task:

```
The Open Problem  →  GROW — append a manual plan stage to the same task,
                     which is the existing research-to-plan path
```

De-escalation runs the ladder downward, is both rarer and safer than
escalation, and — unlike backwards escalation — is already legal, because a
stage the task has not entered may be removed.

## Direction decides the mechanism

There are two moves, and which applies is decided by **direction** — whether the
missing stage belongs ahead of where the task is, or behind it. Each half reads
as a contradiction of the other when stated alone, so here are both.

**Work that needs something AHEAD of where it is grows the same task.** Append
the stage; do not start anything new. This is the existing
research-gains-a-plan-stage path: when the owner chooses an outcome, the task
manager appends a manual `plan` stage to the *same* task with
`kanna_replace_task_workflow`, leaving every existing stage byte-for-byte
intact. That path exists precisely so the research task's durable record — its
prompt, its input ledger, its run history — is what the plan is built on.
`kanna_replace_task_workflow`'s contract permits exactly this and no more:
current and historical stage names must remain with the same role and relative
order, while names absent from history and not currently occupied may be added,
removed, renamed, or reordered.

**Work that needs something BEHIND it becomes a new task from the branch.** Do
not invent an earlier stage, and do not try to re-enter one. Create a new task
with `kanna_create_task`, passing the current task's branch as `base_ref` and
the workflow that should have been chosen in the first place, then retire the
old task. The owner's reasoning, which is the rule:

> I think it makes sense to be able to change your workflow... That being said,
> it's probably easier to realize that the workflow that was created was
> incorrect and simply make a new task based on the task's branch and launch the
> desired workflow that should have been made in the first place. That, I would
> say, is a cleaner way to do it than making up earlier stages.

**A task never escalates backwards.** No engine change is required for any of
this: `kanna_create_task` with `base_ref` already does it today, and the
committed work carries across because the branch does.

### The cost of a replacement task, stated plainly

A replacement task does **not** inherit the original's `task_input` ledger. Any
owner or reviewer directive delivered to the old task mid-flight is invisible to
the new one — and the new task's agents will read its prompt as the complete
terms, because as far as their durable record is concerned, it is.

So whoever makes the replacement must read `kanna_task_inputs` on the old task
**first**, and carry every still-binding directive into the new prompt
**verbatim**. This is not bookkeeping. Losing a directive this way is how a
review agent once ordered an owner's mid-task design decision reverted: the
directive had been issued and delivered, and nothing the reviewer could read
contained it.

That cost is also the argument for why the forward direction grows the task
rather than replacing it. Growing preserves the terms automatically; replacing
re-states them, and re-stating is a step a person can get wrong.

## What already works this way

- `plan-build-review`'s review stage routes findings to `plan` or to
  `in progress` by their nature — level movement inside one pinned definition,
  between stages that definition already has.
- The `research` → append-a-`plan`-stage path grows one task across a level
  boundary while preserving its prompt, input ledger, and run history, which is
  precisely the property the forward direction needs: **the task's terms must
  survive the move.**

## Escalation per pattern, in one table

| Pattern | Enter at | Escalation trigger (observed) | Escalates to | Direction |
|---|---|---|---|---|
| Mechanical Change | `no-review` | the place was not the place | Shaped Delivery | grow, if only review is missing; new task, if a shape must be decided |
| Reported Fault | `no-review`, reproduce first | no reproduction, or it reproduces elsewhere | Open Problem | new task |
| Shaped Delivery (cheap) | `single-reviewer` | review finds the approach, not the code, is wrong | Shaped Delivery (gated) | new task |
| Shaped Delivery (gated) | `plan-build-review` | plan cannot converge on one shape | Open Problem | new task |
| Open Problem | `research` | answer is measured in quarters | Engineering Rewrite | neither — not a task |
| Exploratory Idea | timeboxed probe | finds a real problem (this is an *exit*, not an escalation) | Open Problem / Shaped Delivery | new task, with the accountable origin named |
| Sales-Driven Feature | record, then the uncertainty pattern | origin does not escalate; the prediction is *revisited* | — | — |
| Cross-Team Request | record, then the uncertainty pattern | a second consumer appears | Shaped Delivery, as an interface | new task |
| Forced Move | `no-review` | no compliant minimal change exists | Open Problem (under a deadline) | new task |
| Engineering Rewrite | — | entry should be hard; bias against | — | — |

Every escalation in that table is either a new task or not a task at all, with
one exception — the mechanical change that turns out to want a review stage,
which is the only rung where the missing stage sits ahead. The routine *forward*
move is not escalation at all: it is `The Open Problem` resolving into
`The Shaped Delivery` once an outcome is chosen, and it grows the same task.

---

# Open questions

These are left open on purpose. This draft resolves none of them by fiat.
Question 3 has since been answered by the owner and is marked as answered
rather than removed; the other nine stand.

1. **Does origin belong on the task as data, or only in the prompt?** The claim
   above is that origin selects the accountability artifact. But three of the
   four origin patterns reduce to "write a record and then do the work", and a
   record could be an ordinary document. Kanna may need an origin field, a tag,
   or nothing at all.

2. **Where does a decision record live?** Candidates: a new durable table; a
   committed markdown file under `docs/`; a closed task that is never deleted.
   The prediction field is what makes this hard — something eventually has to
   *re-read* the record at the predicted time, and nothing in Kanna does
   deferred re-reading of anything.

3. ~~**Can a task escalate into a stage that sits ahead of the one it is
   in?**~~ **Answered by the owner, 2026-09-19.** It does not. A task never
   escalates backwards: work needing a stage that belongs earlier becomes a new
   task from the current task's branch, running the workflow that should have
   been chosen, while work needing a stage ahead of where it is grows the same
   task. The question is kept rather than deleted because it is a natural one to
   re-ask — replacement *can* add an unoccupied stage name, so inventing an
   earlier stage looks mechanically available, and the answer is that it is
   cleaner not to. See "Direction decides the mechanism", including the
   `task_input` ledger cost a replacement task carries.

4. **Who is the external forcing function at this company's scale?** The
   Engineering Rewrite pattern assumes a party outside engineering with standing
   to say no. Here, the owner is the entire outside. The `architect` agent is
   bounded and independent of the proposer, which is most of the property — but
   an agent evaluating a two-year human commitment is a different claim than an
   agent evaluating one branch. The pattern may be correct in general and
   unusable at this scale, which is worth saying rather than hiding.

5. **Is `The Reported Fault` a real pattern, or is it `The Mechanical Change`
   with "reproduce first" attached?** Argument for merging: its response is
   almost entirely "find out which pattern you are actually in". Argument for
   keeping: it is the single most common thing that arrives at the door, the
   origin distinction inside it is genuinely load-bearing (the reporter is the
   acceptance test), and a pattern language that cannot name a bug report is
   missing something obvious.

6. **Who matches the context sentences — the task manager or the human?** They
   are written to be matched by an agent at task creation. If the human is the
   real reader, they should be shorter and blunter. If the agent is, they may
   need to be closer to matchable predicates than to prose.

7. **Does the pattern name go on the task?** A visible pattern name is arguable —
   somebody can say "that's not a Shaped Delivery, that's an Open Problem", which
   is the conversation this document exists to enable. An invisible one in the
   manager's head is cheaper and teaches nobody.

8. **Should escalation be budgeted the way revisions are?** `revision_limit`
   defaults to 5 and exists to stop an agent driving a task through endless
   revise/review rounds. Escalation has exactly the same shape and exactly the
   same abuse — a task that escalates four levels has probably been
   misunderstood rather than correctly re-classified four times.

9. **Can `specialized-reviewers` be deleted?** If review depth is an escalation
   rather than a selection, the specialty panel becomes something a review stage
   escalates *into* when it finds something it cannot judge, rather than a
   workflow chosen up front. That would remove one of the four public workflows
   and replace it with a trigger.

10. **Is ten patterns already too many?** The owner's constraint is that a
    catalogue nobody can pick from is worse than four workflows. Ten is inside
    the stated 8–12, but the five uncertainty patterns are the ones that
    actually route work; the four origin patterns and the artifact could be
    argued down to "write a decision record when the decision was not
    engineering's to make". That reduction would leave six, and six that get
    used beats ten that get skimmed.
