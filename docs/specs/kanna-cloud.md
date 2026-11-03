# Kanna Cloud

**Status:** planned; enrollment by token and the `hosted` mark built (slice 9), provisioning being split into Kanna's internal `kanna-cloud` · **Beside:** [the customer's own machines](machines.md) · **Parts:** [installing a host](cloud-provisioning.md), [how Kanna provisions](cloud-vms.md), [build Macs](build-macs.md), [build Mac isolation](build-mac-isolation.md) · **Order:** [build order](machines-build-order.md#kanna-cloud)

**Kanna Cloud is Kanna's product:** a paid service that runs work for its customers on Kanna's servers. A customer uses it from Kanna itself (the app and the `kanna` CLI), signed in to a Cloud account. **They see only tasks:** any task can run on the cloud instead of on one of their machines, and Kanna provisions whatever it runs on. Hosts, VMs and their records are Kanna's internal business, run with Kanna's own tool, `kanna-cloud`, which no customer installs.

**It runs on Kanna's infrastructure, not on [the customer's own machines](machines.md).** Those are the machines the customer decided to trust and that belong to them, in their signed peer log; nothing Kanna runs ever joins them. A Cloud task runs on a VM of Kanna's, which the customer never sees, names or manages, and their Kanna reaches it through Kanna Cloud, signed in with their account. Pairing machines and moving tasks between them is free and needs no Cloud account.

**The problem it solves.** Kanna's agents run on the customer's own machines, which limits them three ways:

- **Capacity:** a laptop runs a few agents at once before it slows down, and a customer without a spare Linux box or Mac has nowhere else to send more.
- **Availability:** a task stops when the laptop sleeps, closes or leaves the network; work queued on the Shelf waits for it.
- **Setup:** running a server of their own means buying it, installing Kanna, keeping it secure and updated, and giving it git and agent credentials by hand; building Apple code from Linux needs a Mac besides.

Kanna Cloud removes all three for a price: the customer sends a task to the cloud and it runs now, on a clean machine of its own, whether their laptop is open or not, with Apple builds on Kanna's Macs. They manage nothing but the task.

It is open source like the rest of Kanna. **A self-hosted Kanna Cloud is the same software run by someone else**: whoever runs it is the operator, and everything below about Kanna as operator applies to them.

## Contents

1. [Rules](#1-rules)
2. [What a customer gets](#2-what-a-customer-gets)
3. [Keys a customer gives Cloud](#3-keys-a-customer-gives-cloud)
4. [How Kanna runs it](#4-how-kanna-runs-it)
5. [Cloud tasks](#5-cloud-tasks)
6. [Enrollment](#6-enrollment)
7. [Directory and relays](#7-directory-and-relays)
8. [Build Macs](#8-build-macs)
9. [Agent sign-in on Kanna-run machines](#9-agent-sign-in-on-kanna-run-machines)
10. [Threat model](#10-threat-model)
11. [Open questions](#11-open-questions)
12. [Glossary](#12-glossary)

## 1. Rules

- **Customers see tasks, not machines.** No customer command reaches a host, names a VM or asks for a machine. Making, setting up and destroying VMs is Kanna's (`kanna-cloud`, and later the provisioning service behind accounts).
- **A VM never holds two customers.** Each is one customer's, from creation to destruction, and is never handed to another.
- **The operator can see the customer's code.** A machine that runs a customer's agent sees their code, their agent's credentials and its transcripts, whoever operates it. End-to-end encryption protects only what passes through the directory and relays.
- **Trust still comes from the customer's devices.** Cloud can withhold but never forge; adding a machine to theirs takes a signature from one of their own devices and a gesture in the app.
- **Kanna-run machines can't change the customer's machines.** A VM Kanna runs is marked `hosted` in their peer log; its key admits and revokes no one but itself (§10).
- **Tenants are isolated by VMs,** Linux and macOS alike, with the network enforced on the host. Host accounts are never the boundary.
- **Cloud covers only what needs servers:** Cloud tasks, the directory and relays, build Macs, and later the phone away from the LAN, push and an always-on Shelf copy. Everything else is the customer's own machines', and free.

## 2. What a customer gets

| Offer | The customer does | Kanna does | The customer sees |
|---|---|---|---|
| **Cloud tasks** | `kanna task new … --cloud`, or moves a task there (`kanna task move TASK --cloud`) | makes a VM for that task alone, runs it there, destroys the VM when the task is done (§5) | the task, on their Desk or Shelf like any other; never the VM |
| **Directory and relays** | signs their machines in to their account | publishes their peer log, carries pairing, relays traffic (§7) | their machines finding each other anywhere |
| **Build Macs** | runs a task (on the cloud or a Linux machine of theirs) that builds Apple code | runs the build on a macOS VM of theirs (§8) | `xcodebuild` working as if local |

## 3. Keys a customer gives Cloud

| Key | For | Where it lives | Who can read it |
|---|---|---|---|
| **Cloud account key** | authenticates their Kanna to the service: requests, billing | the Mac's Keychain; the service keeps a hash | the customer |
| **Git access** | the VM clones and pushes their repo | a deploy key made **in the VM** and added by the customer's Kanna with their own GitHub sign-in; or a fine-grained token they paste | the VM, so the operator |
| **Agent account** | the agent runs on their own Claude or Codex usage | supplied as to any of their machines ([agent sign-in](agent-sign-in.md)); a Codex ChatGPT login stays on their Mac and the VM gets short-lived tokens | the VM, so the operator, for a setup token or API key (§9) |

Kanna never holds the customer's GitHub sign-in: their own Kanna adds and removes the deploy keys.

## 4. How Kanna runs it

```mermaid
flowchart LR
    subgraph YOU["the customer's own machines"]
        MBP["Kanna.app + daemon<br/>mbp"]
        OWN["their own Linux / Macs"]
    end
    subgraph K["Kanna Cloud (the operator's)"]
        DIR["directory<br/>accounts · published peer logs<br/>PAKE relay"]
        REL["relays"]
        subgraph R720["R720 hosts · Ubuntu + Incus"]
            VA["user A's VMs"]
            VB["user B's VMs"]
        end
        subgraph BM["build Mac hosts"]
            MA["A's macOS VM"]
            MB["B's macOS VM"]
        end
    end
    MBP <-.->|"directory, relayed or direct"| DIR & REL
    OWN <-.-> REL
    REL <-.-> VA
    VA ==>|"build jobs"| MA
    VB ==>|"build jobs"| MB
    classDef kanna fill:#2f6f4f,stroke:#6fcf97,color:#ffffff
    classDef os fill:#4a4a55,stroke:#9a9aa8,color:#ffffff
    class MBP,OWN,VA,VB,MA,MB kanna
    class DIR,REL os
```


| Piece | Runs on | Is |
|---|---|---|
| Tenant VMs | R720 hosts, Incus | the Linux VM one Cloud task runs on |
| Provisioning service | Kanna's servers | accounts and account keys; takes requests, makes and releases VMs on hosts (before it exists, `kanna-cloud` by hand) |
| Directory | Kanna's servers | each account's published peer log, relay URLs and addresses; carries PAKE messages |
| Relays | Kanna's servers | `iroh-relay`, the same as one a customer runs themselves |
| Build Macs | Kanna's Macs | tenant-exclusive macOS VMs that run Linux tasks' Apple builds |

**Provisioning, internal** ([how Kanna provisions](cloud-vms.md)):

1. **A host:** `scripts/cloud/provision-idrac.sh` installs Ubuntu Server on an R720 through its iDRAC ([installing a host](cloud-provisioning.md)); `kanna-cloud setup-host` adds Incus, the tenant bridge, its ACL and profile, and the pool.
2. **A VM:** `kanna-cloud provision` makes an Incus VM (Ubuntu 26.04, stock cloud image) for one Cloud task, runs [server setup](machine-setup.md) in it, and has it wait to be enrolled with the request's token (§6).
3. **Releasing** destroys it with its disks, snapshots, logs and network state; nothing of it is reused (§10).

- **Networking:** a tenant VM reaches the internet and relays, never another tenant's VM, the host's management or Kanna's LAN; enforced on the host, IPv4 and IPv6. Customers reach their VMs through iroh: direct when it can punch through, else relayed.
- **Before the service exists** the customer's request and Kanna's answer are passed by hand: sending a task to the cloud prints a request (its token), Kanna runs `kanna-cloud provision --request …`, and gives back the VM's key, which the customer's Kanna enrolls and moves the task to. The service carries the same two messages later.

## 5. Cloud tasks

**Being reconsidered** (review c2): this section and §6 still enroll the task's VM into the customer's peer log; they are rewritten once the link to Kanna Cloud below is agreed.

A task the customer sends to Cloud runs on a VM made for it alone, which they never see.

- **The VM joins their machines,** enrolled by token (§6) and marked `hosted` and `task`, so the existing links, Shelf records and [task transfer](task-transfer.md) carry the task there and its work back. The app doesn't list it in the Machines tab, and `kanna machine list` leaves it out; the task shows where it is as `cloud`.
- **The task's repo** reaches the VM through git access (§3), cloned there before the task moves; the task's branch comes with the transfer.
- **Done, stopped for good or moved off:** the VM's work is transferred back (or pushed, by the task), the customer's Kanna revokes its key and removes its deploy keys, and Kanna destroys the VM.
- **The gesture:** a Cloud task asks once, when the customer first sends tasks to Cloud from a Mac, not per task: the app approves `task` VMs made for this account's requests, which the account key authenticates (§11).

## 6. Enrollment

A Cloud task's VM joins the customer's machines with a one-time token their own Kanna made when it sent the task, instead of a typed code. The customer's own machines join a Cloud account by the usual code pairing ([identity §2](machine-identity.md#2-pairing)), with the directory carrying the PAKE so they needn't share a LAN; machines that move to Cloud publish the log they have, and nothing is paired again.

```mermaid
sequenceDiagram
    participant A as Kanna.app + daemon (mbp)
    participant P as Kanna provisioner
    participant V as new VM
    participant D as directory
    A->>P: provision a VM, with a one-time token T
    P->>V: make the VM, cloud-init with T, run server setup
    V->>V: first boot makes device.key, offers pair with T
    V->>D: register, waiting to pair
    A->>D: pair stream to V, through the directory
    A->>V: SPAKE2 over T, proofs both ways
    A->>A: Cloud tasks approved on this Mac?
    A->>V: the peer log with A's signed add {hosted: true}
    A->>D: publish the log
```

- **Different trust from the LAN.** On the LAN the customer is the only party to pairing. Here Kanna wrote T into the VM, so the PAKE protects the VM's enrollment from everyone except the operator, who runs the VM anyway. What still holds: the operator can't add any other key to the customer's machines, since an add needs a non-hosted device's signature and their gesture.
- **The gesture** is the one approval of Cloud tasks on that Mac (§5): the customer's Kanna made the token for a task they sent, so it adds that VM without asking again. Nothing else is ever added without a gesture.

## 7. Directory and relays

- **The directory** holds each account's published peer log (signed by the customer's devices, so it can withhold but not forge), each peer's relay URL and last addresses, and relays the PAKE for machines not on one LAN.
- **Relays** are `iroh-relay`, as for a relay a customer runs; Kanna's are run for all tenants.
- **Later, on the directory:** an always-on copy of the Shelf, encrypted with the customer's keys, so a task added on a closed laptop reaches the cloud; push notifications; the phone away from the LAN. About $5 a month, as decided before; VM and build-Mac time are priced with billing.
- **A scheduler** can own every waiting task of a user's VMs and assign them centrally instead of runners asking ([tasks §2](machine-tasks.md#2-owners-and-grants)).
- **Placement by capability.** Each machine advertises its capabilities (OS, architecture, cores, RAM, Xcode versions, display) in its peer log entry ([identity §3](machine-identity.md#3-the-peer-log)). A task's `needs:` (`needs: xcode`, `needs: linux`) is matched against them, by a runner before it asks or by the scheduler, so a task goes only where it can run. It's Cloud's infrastructure: the app doesn't show capabilities ([links §5](machine-links.md#5-paths-the-machines-tab-and-offline)).

## 8. Build Macs

A Linux task's `xcodebuild`, `xcrun`, `simctl` and chosen `swift` calls run on a build Mac against the agent's worktree, as if Xcode were local, and a remote task's `.app` is fetched to the MBP's Simulator ([build Macs](build-macs.md)). Build Macs form a **pool** shared by tenants, so every build runs in a macOS VM exclusive to one tenant, and the host never runs project code ([isolation](build-mac-isolation.md)). Build Macs are a Cloud feature: the customer's own machines aren't designed around them, and a self-hoster who wants them runs the same pool software, for example in their own Kanna Cloud.

## 9. Agent sign-in on Kanna-run machines

[Agent sign-in](agent-sign-in.md) belongs to the customer's own machines: accounts are added once on the Mac and supplied to each machine. On a VM Kanna runs:

- **What sits on the VM, the operator can read:** a Claude setup token or an API key is there in its environment. Short-lived credentials are preferred: Codex's holder model gives the VM only access tokens, with the holder on one of the customer's own devices, never a hosted VM.
- **Released or revoked, its credentials are rotated:** the app asks the customer to rotate a long-lived token or key a released VM held, and revokes its deploy key.

## 10. Threat model

| Risk | On a machine the customer runs | On a VM Kanna runs |
|---|---|---|
| Who holds `device.key` | the customer's daemon, `0600` in its state folder | the VM's daemon, made on its first boot inside the VM; the provisioner never sees it |
| Who can sign with it | the customer and their agents (a single-tenant risk: [identity §4](machine-identity.md#4-risks)) | also the operator: host root reads a guest's disk and memory, and the R720s' CPUs have no memory encryption to stop it |
| What a signature can do | add and revoke peers (with a gesture) | nothing beyond being that VM: a `hosted` key's adds and revokes of others don't count |
| The customer's code and credentials | theirs | visible to the operator (§1, §3) |
| Reassignment | — | never: a released VM is destroyed with its disks and snapshots, the customer's app revokes its key, and the next customer gets a new VM with a new key |
| Other tenants | none | VMs and host-enforced networks between them (§4); build Macs per [isolation](build-mac-isolation.md) |
| The Cloud account key | — | lets whoever holds it ask for VMs on the account and be billed; it can't add a machine to the customer's, which still takes their gesture |

- **A prompt-injected agent on a hosted VM** can read its `device.key` too; the `hosted` rule means it can't admit a peer either. Claude's settings deny reading the key; Codex, run with `--yolo`, can read it ([identity §4](machine-identity.md#4-risks)).
- **The directory** is the operator's: it can withhold peers or delay revocations, which the customer's devices also carry to each other directly. It can't forge an add.

## 11. Open questions

- **What a hosted VM may run on the customer's other machines.** It can't change their machines (§10), but as one of them it can run commands and tasks on their Mac (`kanna --host mbp …`), as their Mac can on it, so its operator could too. **Proposed:** hosted peers reach only their own tasks' records and streams, never other machines' terminals or commands. It comes before Cloud tasks and before a second tenant.
- **Cloud tasks without a gesture each.** **Proposed:** the customer approves once per Mac that `task` VMs made for their account's requests join without a sheet, since their own Kanna asked for each (it made the token).
- **Where a Cloud task's work lands** when the customer's Macs are all closed. **Proposed:** the task pushes its branch with its git access, and the transfer back happens when a Mac next answers.
- **A copy of agent secrets in the directory**, so a VM starts while the Mac is closed (the earlier plan). **Proposed:** none for now; needed for Cloud tasks with the Mac closed, together with the encrypted Shelf copy.
- **The first tenant:** **Decided:** Cloud's VMs go on the host-enforced tenant network (§4) from the first; the network check runs on each host before a second tenant.
- **A customer's own Mac as their private build Mac.** **Proposed:** not in the first Cloud milestone; the pool is Kanna's Macs.
- **Billing:** VM and build-Mac time per account, with the account key; the directory's $5 a month as decided before.

## 12. Glossary

| Term | Meaning |
|---|---|
| **Operator** | Whoever runs a Kanna Cloud: Kanna, or anyone self-hosting it |
| **Customer** | Someone with a Cloud account, using it from their own Kanna |
| **Cloud task** | A customer's task run on the cloud: on a VM made for it alone, which they never see, destroyed when the task is done |
| **Cloud account key** | What authenticates a customer's Kanna to the service |
| **`kanna-cloud`** | Kanna's internal operator CLI: hosts, VMs, releases; never shipped to customers |
| **Tenant** | One customer (an account), owner of their VMs and peer log |
| **Hosted peer** | A machine in a customer's peer log that the operator runs, marked `hosted` when they add it |
| **Directory** | Cloud's service that publishes each account's peer log and relays pairing |
| **Enrollment token** | The one-time code a provisioned VM pairs with: made by the customer's Kanna, put in the VM by the provisioner |
| **Incus** | The VM and container manager on the R720 hosts |
