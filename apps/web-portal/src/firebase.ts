import { initializeApp } from "firebase/app";
import {
  connectAuthEmulator,
  createUserWithEmailAndPassword,
  getAuth,
  onAuthStateChanged,
  sendEmailVerification,
  sendPasswordResetEmail,
  signInWithEmailAndPassword,
  signOut,
  type User
} from "firebase/auth";
import { connectFirestoreEmulator, doc, getDoc, getFirestore, onSnapshot } from "firebase/firestore";
import { connectFunctionsEmulator, getFunctions, httpsCallable } from "firebase/functions";
import type { CheckoutSessionRequest, CheckoutSessionResponse, PortalSessionRequest, PortalSessionResponse } from "@kanna/firebase-functions/billing-contract";
import type { CloudEntitlement } from "./types";

function required(name: keyof ImportMetaEnv): string {
  const value = import.meta.env[name]?.trim();
  if (!value) throw new Error(`Missing required portal configuration: ${name}`);
  return value;
}

const app = initializeApp({
  apiKey: required("VITE_FIREBASE_API_KEY"),
  authDomain: required("VITE_FIREBASE_AUTH_DOMAIN"),
  projectId: required("VITE_FIREBASE_PROJECT_ID"),
  appId: required("VITE_FIREBASE_APP_ID")
});

export const auth = getAuth(app);
export const db = getFirestore(app);
export const functions = getFunctions(app, import.meta.env.VITE_FIREBASE_FUNCTIONS_REGION || "us-central1");

if (import.meta.env.VITE_FIREBASE_USE_EMULATORS === "true") {
  connectAuthEmulator(auth, `http://127.0.0.1:${import.meta.env.VITE_FIREBASE_AUTH_EMULATOR_PORT || "9099"}`, { disableWarnings: true });
  connectFirestoreEmulator(db, "127.0.0.1", Number(import.meta.env.VITE_FIREBASE_FIRESTORE_EMULATOR_PORT || "8080"));
  connectFunctionsEmulator(functions, "127.0.0.1", Number(import.meta.env.VITE_FIREBASE_FUNCTIONS_EMULATOR_PORT || "5001"));
}

export const portalFirebase = {
  observeUser(callback: (user: User | null) => void): () => void {
    return onAuthStateChanged(auth, callback);
  },
  async register(email: string, password: string): Promise<User> {
    const credential = await createUserWithEmailAndPassword(auth, email, password);
    await sendEmailVerification(credential.user);
    return credential.user;
  },
  async signIn(email: string, password: string): Promise<User> {
    return (await signInWithEmailAndPassword(auth, email, password)).user;
  },
  signOut(): Promise<void> {
    return signOut(auth);
  },
  resetPassword(email: string): Promise<void> {
    return sendPasswordResetEmail(auth, email);
  },
  resendVerification(user: User): Promise<void> {
    return sendEmailVerification(user);
  },
  async reloadUser(user: User): Promise<User> {
    await user.reload();
    if (auth.currentUser !== user) throw new Error("The signed-in account changed. Please try again.");
    // reload updates emailVerified locally; callables need the refreshed claim too.
    await user.getIdToken(true);
    if (auth.currentUser !== user) throw new Error("The signed-in account changed. Please try again.");
    return user;
  },
  observeEntitlement(
    uid: string,
    next: (entitlement: CloudEntitlement | null, fromCache?: boolean) => void,
    error: (error: Error) => void,
  ): () => void {
    return onSnapshot(doc(db, "users", uid, "entitlements", "cloud_access"),
      { includeMetadataChanges: true },
      (snapshot) => {
        if (snapshot.metadata.fromCache) {
          // Initial/cache-only snapshots are pending, not a failed read or proof of access.
          next(null, true);
          return;
        }
        next(snapshot.exists() ? snapshot.data() as CloudEntitlement : null);
      }, error);
  },
  async createPortalSession(): Promise<PortalSessionResponse> {
    const callable = httpsCallable<PortalSessionRequest, PortalSessionResponse>(functions, "createPortalSession");
    return (await callable({})).data;
  },
  async entitlement(uid: string): Promise<CloudEntitlement | null> {
    const snapshot = await getDoc(doc(db, "users", uid, "entitlements", "cloud_access"));
    return snapshot.exists() ? snapshot.data() as CloudEntitlement : null;
  },
  async createCheckoutSession(request: CheckoutSessionRequest): Promise<CheckoutSessionResponse> {
    const callable = httpsCallable<CheckoutSessionRequest, CheckoutSessionResponse>(functions, "createCheckoutSession");
    return (await callable(request)).data;
  },
  async deleteAccount(): Promise<void> {
    const callable = httpsCallable<Record<string, never>, { deleted: true }>(functions, "deleteAccount");
    await callable({});
  }
};

export type PortalFirebase = typeof portalFirebase;
