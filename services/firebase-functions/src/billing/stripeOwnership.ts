/**
 * Kanna's Stripe business account also bills Kanji Kongbu. Nothing here is
 * about a second Stripe account or a new billing implementation: it is the
 * proof, checked before any write or provider mutation, that a Stripe object
 * (a subscription, a checkout session, or everything found on a shared
 * customer) actually belongs to the Kanna Cloud product before Kanna acts on
 * it or lets a caller act through it.
 *
 * Line items are read from Stripe rather than trusted from a webhook payload:
 * checkout/invoice payloads can omit product details entirely, and a payload
 * cannot prove what it does not carry.
 */

/** Verdict for one Stripe object (a subscription or a checkout session). */
export type ProductOwnershipVerdict = "owned" | "foreign" | "ambiguous";

/**
 * Classify a single object's line-item product ids against the expected Kanna
 * product.
 *
 * No resolvable product id (a lookup failure that returned nothing, or a
 * vanished object) and more than one distinct product on the same object both
 * come back `ambiguous`: neither proves Kanna ownership, and a mixed-item
 * object must never be treated as ours just because one of its lines is.
 */
export function classifyProductOwnership(
  productIds: readonly string[],
  expectedProductId: string
): ProductOwnershipVerdict {
  const unique = new Set(productIds);
  if (unique.size !== 1) return "ambiguous";
  return unique.has(expectedProductId) ? "owned" : "foreign";
}

/** Verdict for everything found on a shared Stripe customer. */
export type CustomerScopeVerdict = "clean" | "owned" | "mixed" | "unresolved";

/** The result of scanning every billing object on a shared Stripe customer. */
export interface CustomerProductScan {
  /** Distinct product ids resolved across every item found. */
  productIds: readonly string[];
  /**
   * True when at least one item on the customer could not be resolved to a
   * product id — a paginated lookup failure, or an item whose price/product
   * could not be read. This must never collapse into the same "nothing here"
   * result as a customer with genuinely zero billing.
   */
  unresolved: boolean;
}

/**
 * Classify a customer-wide operation (Portal, deletion recovery) against the
 * expected Kanna product.
 *
 * Unlike a single object, a customer legitimately has no billing at all (a
 * comped account, a canceled subscription) — that is `clean`, not ambiguous,
 * because there is nothing here that isn't Kanna's to expose. Any foreign
 * product id anywhere on the customer makes the whole customer `mixed`, which
 * blocks a customer-wide action rather than trying to partition it. A scan
 * that could not resolve every item is `unresolved` — never treated as clean,
 * because an unresolved item might be a foreign product this scan simply
 * failed to identify.
 */
export function classifyCustomerScope(
  scan: CustomerProductScan,
  expectedProductId: string
): CustomerScopeVerdict {
  if (scan.productIds.some((id) => id !== expectedProductId)) return "mixed";
  if (scan.unresolved) return "unresolved";
  return scan.productIds.length > 0 ? "owned" : "clean";
}
