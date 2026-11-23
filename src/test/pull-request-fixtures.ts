import type { Connection, PublishReadiness, PullRequest, PullRequestLink } from "@/lib/api";

export function makePullRequest(overrides: Partial<PullRequest> = {}): PullRequest {
  return {
    number: 318,
    title: "Adds the refund flow",
    description: "Refunds settled payments.",
    state: "open",
    url: "https://bitbucket.org/acme/payments-api/pull-requests/318",
    author: "Ana",
    sourceBranch: "feat/refund-flow",
    sourceCommit: "e7f3b10c9d8e",
    destinationBranch: "main",
    destinationCommit: "0123456789ab",
    participants: [],
    ...overrides,
  };
}

/** The connected account in `makeLinked`. */
export const ME = { id: "{me}", name: "Dana" };

/**
 * A pull request found and ready to publish to and to approve, unless `reason` says why not (for both, unless
 * `verdictReason` is given).
 */
export function makeLinked(
  reason: string | null = null,
  pullRequest: Partial<PullRequest> = {},
  verdictReason: string | null = reason,
): PullRequestLink {
  return {
    kind: "linked",
    hostName: "Bitbucket Cloud",
    pullRequest: makePullRequest(pullRequest),
    account: ME,
    publish: readiness(reason),
    verdict: readiness(verdictReason),
  };
}

/** A reason given as its English text becomes a publish_blocked error without a reason key. */
function readiness(reason: string | null): PublishReadiness {
  return { allowed: reason === null, reason: reason === null ? null : { code: "publish_blocked", message: reason } };
}

export function makeConnection(overrides: Partial<Connection> = {}): Connection {
  return {
    host: "bitbucket",
    hostName: "Bitbucket Cloud",
    scopes: ["read:user:bitbucket", "read:pullrequest:bitbucket", "write:pullrequest:bitbucket"],
    needsUsername: true,
    username: null,
    account: null,
    connectedAt: null,
    ...overrides,
  };
}

/** GitHub, not connected: its token alone says who it is. */
export function makeGithubConnection(overrides: Partial<Connection> = {}): Connection {
  return makeConnection({
    host: "github",
    hostName: "GitHub",
    scopes: ["repo"],
    needsUsername: false,
    ...overrides,
  });
}
