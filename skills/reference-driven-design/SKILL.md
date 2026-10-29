---
name: reference-driven-design
description: Always load.
---

# Reference-driven design
Instead of endless specifications that inevitably drift from the actual codebase, we make simple mockups (static html or interactive web app) or prototypes (a non-performant version in python) or a utility that sets the standard for performance that capture the characteristics (design, behaviour, or performance) of the design and we use that as a long-living reference for the product.

## How to know what we're building

1. Start as high-level as possible.
1. Make the model (mockup, prototype, etc)
1. Iterate with the user until the meaningful characteristcs (design/behaviour/performance) is captured.
1. Create a brief specification doc that covers behaviour etc captured by the model. Use screen grabs from the model to illustrate points. Items not covered by the model need to be determined with the user or indicate the model itself is insufficient.
1. Iterate with the user to flesh out the specification for items that are unclear.

## How to build

Don't build A, then B, then C and hope it all works when you plug them together. It never does. If there's one thing you learn in engineering school is that integration is the painful step, so make it the first step.

Start high level. Scaffold building blocks. Define interfaces and abstractions. Mock internals. Connect things top down. Make meaningful tests. Take babysteps. Add the minimal next step towards implementing the model's characteristics. Commit after each step. As characterictics span building blocks, each step should be implementing support for the feature across building blocks.

Iterate this process while comparing behaviour against the reference model.

## Keeping the model

**One model per feature.** A new feature gets its own small mockup (`references/mockups/<feature>.html`) that copies only the look it needs from the others, rather than more scenes and buttons in an existing mockup, which grows hard to follow.

Models live in the repo's `references/` folder, never in `docs/`: mockups in `references/mockups/`, prototypes in `references/prototypes/<name>/`, and reference implementations (another codebase the product follows) in `references/<name>/`. Specs link to them there.

The model is never retired once the product implements it; it stays the reference the product is compared against. When a behaviour or rendering fix lands in the product, port it to the model so both keep behaving alike.

The model is not production code. Don't harden it: security findings in a model (including automated security reviews) get acknowledged in a line and handled in the product, not fixed in the model.
