# 0018. Recipient-key documents need a trusted ACL owner

- Status: Accepted
- Date: 2026-10-12

## Context
Anyone can wrap a key to a public X-Wing key. In documents with an ACL
chain, keyrings must be signed by an owner or admin (spec §6.4, §8.4), so
nobody else can slip in a key they know. But a reader decided that a
document "has an ACL chain" from the file itself. Whoever can write to the
file (a Hub, a sync service, or anyone the file passes through) could
therefore:

- remove every `ACLC` record and add an unsigned keyring that wraps a key
  of their choice to the reader's public key, or
- replace the chain with a new one started by their own identity, which
  is internally valid.

In both cases the reader opens a forged document as genuine, and anything
it writes next is encrypted under a key the attacker knows. Found in the M1
code review.

Password, folder and link wraps are not affected, because creating them
needs the secret. Only recipient (HPKE) wraps are.

## Decision
- **HPKE wraps count only under an ACL chain started by an expected
  owner.** A reader that unlocks with a recipient key checks that the
  document has an ACL chain and that its `Genesis` was signed by the
  identity it expects. The keyring must then be authorized by that chain
  (§8.4). Documents without a chain, or with a chain started by anyone
  else, don't open with recipient keys.
- **The expected owner comes from outside the file:**
  - the user's own identity, for documents protected with account keys
    (ADR-0016);
  - the invitation, share link or Hub directory entry for shared
    documents, remembered on first open (trust on first use, ADR-0011).
- Documents protected with account keys therefore always carry an ACL
  chain, at least a `Genesis` by the user.
- The anchor is the **Genesis owner's key id**, not the current owner, so
  a transfer of ownership doesn't break it.
- In the API, `RecipientKeys::new` takes the expected owner.
  `RecipientKeys::trusting_any_owner` exists for inspection tools such as
  `imag`, which report the owner instead of checking it.

## Consequences
- Account-key files grow by about 11 KiB: the `Genesis` (7.8 KiB with the
  hybrid identity key, the device keys and the signature, measured on
  vector 10a) and a hybrid signature on each keyring (3.4 KiB).
- Every caller that opens with a recipient key has to know whom it trusts.
  For the app that is the account identity, or a value stored with the
  document's entry in the local library.
- Vector 08 (HPKE wraps without a chain) still documents the wrap itself,
  but the file no longer opens. Vector 10a shows the accepted form.
- Rolling a chain back to an older head isn't detected by this: it needs
  remembered state per document (see spec §12).

## Alternatives considered
- **A header flag "this document is shared".** It is unauthenticated, so
  it can't stop a chain from being replaced, and it would need a format
  change.
- **Pinning the genesis hash instead of the owner.** That is stronger
  (one exact chain), but a user's devices would have to remember the
  genesis of every account-key file. The owner's key id is known up front,
  and an attacker can't produce a `Genesis` signed by it.
- **Refusing recipient wraps only when no chain exists.** That leaves the
  "replace the chain" attack open.
