# Kanna account portal

Vue/Firebase account and Stripe Checkout funnel for Kanna Cloud. Local browser configuration is documented in `.env.example`; copy it to an untracked `.env.local` and use the Firebase emulators for development.

`./kd cloud deploy --staging` builds and deploys this app with the rest of the Firebase surface. Cloud deploys need three values:

- `KANNA_WEB_PORTAL_FIREBASE_API_KEY`
- `KANNA_WEB_PORTAL_FIREBASE_APP_ID`
- `KANNA_WEB_PORTAL_STRIPE_PUBLISHABLE_KEY`

**An operator does not have to supply them.** They are committed per Firebase project in `.env.kanna-build` and `.env.kanna-staging`, which `kd` reads as a layer beneath its own environment — the same `.env.<projectId>` shape `services/firebase-functions` already uses. A deploy needs nothing in the deployer's shell; exporting one of these variables still overrides the file for that run, and a key present in neither still fails the deploy with an error naming the file it looked in. Vite never loads these files itself (it reads `.env.[mode]`, and the build's mode is `production`), so `kd` is their only reader.

Optional overrides are `KANNA_WEB_PORTAL_FIREBASE_AUTH_DOMAIN`, `KANNA_WEB_PORTAL_FIREBASE_FUNCTIONS_REGION`, and `KANNA_WEB_PORTAL_CLOUD_PRICE`; both committed files leave all three unset so the resolver's defaults (`${projectId}.firebaseapp.com`, `us-central1`, and `DEFAULT_WEB_PORTAL_CLOUD_PRICE`) stand.

These are public browser identifiers/display configuration, not server secrets: Vite compiles them into the bundle and Hosting serves them to every visitor, which is why committing them is safe. Stripe secret keys and `STRIPE_WEBHOOK_SECRET` remain in the Functions secret store and must never enter a repository file.

The integration command starts its own Auth emulator. Until the billing backend lands, the test harness serves a test-only `createCheckoutSession` callable stub on the reserved Functions port. The harness uses this worktree's reserved `KANNA_FIREBASE_*_PORT` values and falls back to the standard Firebase emulator ports outside a Kanna task:

```sh
pnpm --dir apps/web-portal test:integration
```
