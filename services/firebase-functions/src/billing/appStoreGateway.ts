import { AppStoreServerAPIClient, Environment } from "@apple/app-store-server-library";
import { appStoreConfig, APP_STORE_SECRET } from "./appStoreConfig.js";
import { requireEnv } from "./config.js";
import { appleRetry, appleStatus, invalidApple, type AppleEvidence, type AppleTransaction, type AppleVerifier, type AppleEnvironment } from "./appStoreVerification.js";

export interface AppleGateway {
  current(transaction: AppleTransaction): Promise<AppleEvidence[]>;
  notificationHistory(environment: AppleEnvironment, originalId: string, startDate: number, endDate: number): AsyncIterable<string>;
}

export function createAppleGateway(env: NodeJS.ProcessEnv, verifier: AppleVerifier): AppleGateway {
  const config = appStoreConfig(env);
  const client = (environment: AppleEnvironment) => new AppStoreServerAPIClient(
    requireEnv(env, APP_STORE_SECRET), requireEnv(env, "APP_STORE_KEY_ID"), requireEnv(env, "APP_STORE_ISSUER_ID"),
    config.bundleId, environment === "production" ? Environment.PRODUCTION : Environment.SANDBOX);
  return {
    async current(transaction) {
      const response = await client(transaction.environment).getAllSubscriptionStatuses(transaction.originalId).catch(() => { throw appleRetry(); });
      if (response.bundleId !== config.bundleId || response.environment !== (transaction.environment === "production" ? Environment.PRODUCTION : Environment.SANDBOX)
        || (transaction.environment === "production" && response.appAppleId !== config.appId)) throw invalidApple();
      const results: AppleEvidence[] = [];
      for (const group of response.data ?? []) {
        if (group.subscriptionGroupIdentifier !== config.groupId) continue;
        for (const item of group.lastTransactions ?? []) {
          if (!item.signedTransactionInfo || !item.signedRenewalInfo) throw invalidApple();
          const pair = await verifier.pair(item.signedTransactionInfo, item.signedRenewalInfo, transaction.environment);
          if (pair.transaction.token !== transaction.token || pair.transaction.originalId !== item.originalTransactionId) throw invalidApple();
          results.push({ ...pair, status: appleStatus(item.status),
            signedDate: Math.max(pair.transaction.signedDate, pair.renewal.signedDate) });
        }
      }
      if (!results.some(item => item.transaction.originalId === transaction.originalId)) throw appleRetry();
      return results;
    },
    async *notificationHistory(environment, originalId, startDate, endDate) {
      let paginationToken: string | null = null;
      do {
        const page = await client(environment).getNotificationHistory(paginationToken, { startDate, endDate, transactionId: originalId });
        for (const item of page.notificationHistory ?? []) if (item.signedPayload) yield item.signedPayload;
        paginationToken = page.hasMore ? page.paginationToken ?? null : null;
        if (page.hasMore && !paginationToken) throw appleRetry();
      } while (paginationToken);
    },
  };
}
