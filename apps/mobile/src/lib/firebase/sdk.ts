import { initializeApp, getApps, type FirebaseApp } from "firebase/app";
import {
  connectAuthEmulator,
  createUserWithEmailAndPassword,
  getAuth,
  inMemoryPersistence,
  initializeAuth,
  onAuthStateChanged,
  sendEmailVerification,
  sendPasswordResetEmail,
  signInWithEmailAndPassword,
  signOut as firebaseSignOut,
  type Auth,
  type User
} from "firebase/auth";
import { type CloudAccessSnapshot } from "@kanna/stream-client";
import { doc, getDocFromServer } from "firebase/firestore";
import AsyncStorage from "@react-native-async-storage/async-storage";
import {
  parseMobileFirebaseConfig,
  type MobileFirebaseConfig
} from "./config";
import {
  createDisabledMobileAuthSession,
  createMobileAuthSession,
  type MobileAuthSdk,
  type MobileAuthSession,
  type MobileAuthUser
} from "./auth";
import { createReactNativeAuthPersistence } from "./authPersistence";
import { getConfiguredFirestore } from "./configuredFirestore";

export function createConfiguredMobileAuthSession(
  config: MobileFirebaseConfig = parseMobileFirebaseConfig()
): MobileAuthSession {
  if (!config.app) {
    return createDisabledMobileAuthSession();
  }

  const app = getApps()[0] ?? initializeApp(config.app);
  const auth = initializeMobileAuth(app);
  if (config.authEmulator) {
    connectAuthEmulator(auth, config.authEmulator.url, {
      disableWarnings: true
    });
  }

  return createMobileAuthSession({
    sdk: createFirebaseMobileAuthSdk(auth, app)
  });
}

function initializeMobileAuth(app: FirebaseApp): Auth {
  try {
    return initializeAuth(app, {
      persistence: isReactNativeRuntime()
        ? createReactNativeAuthPersistence(AsyncStorage)
        : inMemoryPersistence
    });
  } catch (error) {
    if (isFirebaseAuthAlreadyInitializedError(error)) {
      return getAuth(app);
    }
    throw error;
  }
}

function isFirebaseAuthAlreadyInitializedError(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    (error as { code?: unknown }).code === "auth/already-initialized"
  );
}

function isReactNativeRuntime(): boolean {
  return (
    typeof navigator !== "undefined" &&
    (navigator as { product?: string }).product === "ReactNative"
  );
}

export function createFirebaseMobileAuthSdk(auth: Auth, app: FirebaseApp): MobileAuthSdk {
  const db = getConfiguredFirestore(app);
  const getCloudEntitlement = async (uid: string): Promise<CloudAccessSnapshot> => {
    try {
      const snapshot = await getDocFromServer(doc(db, "users", uid, "entitlements", "cloud_access"));
      const data = snapshot.data();
      const status = data?.status;
      const graceEndsAt = typeof data?.graceEndsAt === "string" ? data.graceEndsAt : null;
      const active = status === "active" || (status === "grace" &&
        (!graceEndsAt || Number.isNaN(Date.parse(graceEndsAt)) || Date.parse(graceEndsAt) > Date.now()));
      return {
        active,
        status: status === "active" || status === "grace" || status === "expired" || status === "revoked" ? status : "none",
        graceEndsAt,
        currentPeriodEndsAt: typeof data?.currentPeriodEndsAt === "string" ? data.currentPeriodEndsAt : null
      };
    } catch (error) {
      console.error("Could not load cloud entitlement:", error);
      return { active: true, status: "unknown", graceEndsAt: null, currentPeriodEndsAt: null };
    }
  };
  return {
    getCurrentUser: () => mapFirebaseUser(auth.currentUser),
    onAuthStateChanged(listener) {
      return onAuthStateChanged(auth, (user) => listener(mapFirebaseUser(user)));
    },
    async signInWithEmailPassword(email, password) {
      const credential = await signInWithEmailAndPassword(auth, email, password);
      return mapSignedInFirebaseUser(credential.user);
    },
    async createUserWithEmailPassword(email, password) {
      const credential = await createUserWithEmailAndPassword(auth, email, password);
      await sendEmailVerification(credential.user);
      return mapSignedInFirebaseUser(credential.user);
    },
    async reloadUser() {
      if (!auth.currentUser) return null;
      await auth.currentUser.reload();
      return mapFirebaseUser(auth.currentUser);
    },
    async sendEmailVerification() {
      if (!auth.currentUser) throw new Error("Sign in before requesting a verification email.");
      await sendEmailVerification(auth.currentUser);
    },
    getCloudEntitlement,
    async getCloudAccess(uid) {
      const access = await getCloudEntitlement(uid);
      return access.status === "unknown" ? "unknown" : access.active ? "active" : "inactive";
    },
    sendPasswordResetEmail: (email) => sendPasswordResetEmail(auth, email),
    async signOut() {
      await firebaseSignOut(auth);
    },
    async getIdToken(forceRefresh) {
      return auth.currentUser?.getIdToken(forceRefresh) ?? null;
    }
  };
}

function mapFirebaseUser(user: User | null): MobileAuthUser | null {
  if (!user) {
    return null;
  }

  return {
    uid: user.uid,
    email: user.email,
    displayName: user.displayName,
    emailVerified: user.emailVerified
  };
}

function mapSignedInFirebaseUser(user: User): MobileAuthUser {
  return {
    uid: user.uid,
    email: user.email,
    displayName: user.displayName,
    emailVerified: user.emailVerified
  };
}
