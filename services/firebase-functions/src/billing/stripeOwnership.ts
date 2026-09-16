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
export type CustomerScopeVerdict = "clean" | "owned" | "mixed";

/**
 * Classify a customer-wide operation (Portal, deletion recovery) against the
 * expected Kanna product.
 *
 * Unlike a single object, a customer legitimately has no billing at all (a
 * comped account, a canceled subscription) — that is `clean`, not ambiguous,
 * because there is nothing here that isn't Kanna's to expose. Any foreign
 * product id anywhere on the customer makes the whole customer `mixed`, which
 * blocks a customer-wide action rather than trying to partition it.
 */
export function classifyCustomerScope(
  productIds: readonly string[],
  expectedProductId: string
): CustomerScopeVerdict {
  if (productIds.some((id) => id !== expectedProductId)) return "mixed";
  return productIds.length > 0 ? "owned" : "clean";
}
