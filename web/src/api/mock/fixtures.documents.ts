// Typed documents for the mock server, copied from crates/ostra-core/src/doc/fixtures/*.json as the
// server serializes them (every default filled in).
import type { DocumentView, PhaseDoc, PlanDoc, ResearchDoc, SpecDoc } from "../types";

export const researchDoc: ResearchDoc = {
  "approaches": [
    {
      "best_for": "A small, consistent change.",
      "concept": "Add a cancel method beside refund that checks ownership and ACTIVE status, saves CANCELLED, and publishes `order.cancelled`.",
      "cons": [
        "The warehouse consumer must learn a new event type."
      ],
      "name": "New cancel transition in OrderService",
      "precedent": "src/orders/service.ts:OrderService.refund",
      "pros": [
        "Mirrors the refund flow exactly.",
        "Keeps every state change in one service."
      ],
      "recommended": true
    },
    {
      "best_for": "Many future transitions.",
      "concept": "Expose one endpoint that sets any allowed status from a transition table.",
      "cons": [
        "No precedent in the repo.",
        "Moves validation into a table the tests do not cover."
      ],
      "name": "Generic status endpoint",
      "precedent": "new: no precedent found",
      "pros": [
        "Covers future transitions."
      ],
      "recommended": false
    }
  ],
  "areas": [
    "orders",
    "events"
  ],
  "asks": [
    "A customer can cancel their own order while it is ACTIVE.",
    "The warehouse learns about a cancellation without a manual step."
  ],
  "data_flow": [
    {
      "location": "src/orders/routes.ts:refundRoute",
      "step": "`POST /orders/:id/refund` reaches the orders router."
    },
    {
      "location": "src/orders/service.ts:OrderService.refund",
      "step": "The router calls `OrderService.refund` with the session user."
    },
    {
      "location": "src/orders/repository.ts:OrderRepository.save",
      "step": "The service saves the new status."
    },
    {
      "location": "src/events/publisher.ts:publish",
      "step": "The service publishes `order.refunded`, which the warehouse consumer reads."
    }
  ],
  "date": "2026-07-28",
  "dependencies": [
    {
      "kind": "external",
      "name": "@aws-sdk/client-sqs",
      "role": "Carries domain events to the warehouse queue.",
      "version": "3.614.0"
    },
    {
      "kind": "internal",
      "name": "src/events",
      "role": "Wraps queue publishing for every domain event.",
      "version": null
    }
  ],
  "external": [
    {
      "consequence": "A cancellation event must carry the order id as its message group, the way `publish` already sets it for refunds, or SQS rejects it.",
      "fact": "`MessageGroupId` is required for every message sent to a FIFO queue.",
      "source": "https://docs.aws.amazon.com/AWSSimpleQueueService/latest/APIReference/API_SendMessage.html",
      "technology": "Amazon SQS FIFO queues",
      "version": "API version 2012-11-05"
    }
  ],
  "files": [
    {
      "path": "src/orders/service.ts",
      "purpose": "Every order state change goes through OrderService.",
      "symbols": [
        "OrderService.create(input: NewOrder): Promise<Order>",
        "OrderService.refund(id: string, userId: string): Promise<void>"
      ]
    },
    {
      "path": "src/orders/repository.ts",
      "purpose": "Persists orders; the only writer of the orders table.",
      "symbols": [
        "OrderRepository.save(order: Order): Promise<void>"
      ]
    },
    {
      "path": "src/events/publisher.ts",
      "purpose": "Publishes domain events to the warehouse queue.",
      "symbols": [
        "publish(event: DomainEvent): Promise<void>"
      ]
    }
  ],
  "lessons": [
    {
      "area": "orders::OrderService",
      "lesson": "Status changes must go through the service, because the repository does not publish events.",
      "verified": "src/orders/repository.ts has no publish call"
    }
  ],
  "next_steps": [
    "Research the warehouse consumer in the `warehouse` repo."
  ],
  "not_covered": [
    "The web client's order views, which consume the same events."
  ],
  "open_questions": [
    {
      "id": "Q1",
      "multi_select": false,
      "options": [
        {
          "description": "Matches the refund flow, which rejects orders in the wrong state.",
          "label": "No, only before shipping"
        },
        {
          "description": "Needs a new RETURN_REQUESTED state and a warehouse notification.",
          "label": "Yes, as a return request"
        }
      ],
      "question": "May a customer cancel an order after it has shipped?",
      "recommended": 0,
      "tag": "Scope"
    }
  ],
  "patterns": [
    {
      "description": "The refund flow loads the order, rejects a caller who does not own it, checks the current status, then saves and publishes. A cancellation would follow the same order of checks.",
      "files": [
        "src/orders/service.ts"
      ],
      "name": "Ownership check, then transition",
      "snippet": {
        "code": "async refund(id: string, userId: string) {\n  const order = await this.repo.find(id);\n  if (!order) throw new NotFound(id);\n  if (order.userId !== userId) throw new Forbidden();\n  if (order.status !== \"DELIVERED\") throw new InvalidState(order.status);\n  order.status = \"REFUNDED\";\n  await this.repo.save(order);\n  await publish({ type: \"order.refunded\", orderId: id });\n}",
        "language": "ts",
        "source": "src/orders/service.ts:48"
      }
    }
  ],
  "problem": "Customers cannot cancel an order once it is placed. Support staff cancel orders by editing the database row, which skips the order-changed event and leaves the warehouse queue unaware.",
  "recommendation": "Add the cancel transition to `OrderService`, following `refund`.",
  "repo": "backend",
  "scope": "The backend order lifecycle and its event publication, for the task `Research how orders change state and what cancelling one would touch`. The web client's order views were out of scope.",
  "sources": [
    {
      "established": "FIFO sends need MessageGroupId, which settles how the event is published.",
      "url": "https://docs.aws.amazon.com/AWSSimpleQueueService/latest/APIReference/API_SendMessage.html",
      "version": "API version 2012-11-05"
    }
  ],
  "title": "Order cancellation"
};

export const specDoc: SpecDoc = {
  "assumptions": [
    {
      "source": "src/orders/types.ts:OrderStatus",
      "text": "CANCELLED is a new terminal status that no existing flow reads."
    }
  ],
  "contracts_consumed": [
    {
      "name": "Event publisher",
      "shape": "`publish(event: DomainEvent): Promise<void>`",
      "source": "src/events/publisher.ts:publish"
    }
  ],
  "contracts_provided": [
    {
      "consumed_by": [
        "D2"
      ],
      "name": "Cancel order endpoint",
      "provided_by": "D1",
      "shape": "`POST /orders/:id/cancel`, no body. 200 with the order JSON; 403 when the caller does not own it; 409 when it is not ACTIVE."
    }
  ],
  "criteria": [
    {
      "depends_on": [],
      "grounding": "src/orders/service.ts:OrderService.refund",
      "id": "C1",
      "kind": "Functional",
      "provisional": null,
      "repo": "backend",
      "statement": "A customer can cancel their own order while it is ACTIVE."
    },
    {
      "depends_on": [],
      "grounding": "src/orders/service.ts:OrderService.refund",
      "id": "C2",
      "kind": "Constraint",
      "provisional": null,
      "repo": "backend",
      "statement": "Cancelling an order the caller does not own is rejected."
    },
    {
      "depends_on": [
        "C1"
      ],
      "grounding": "https://docs.aws.amazon.com/AWSSimpleQueueService/latest/APIReference/API_SendMessage.html",
      "id": "C3",
      "kind": "Integration",
      "provisional": null,
      "repo": "backend",
      "statement": "A cancellation publishes an event the warehouse can read."
    },
    {
      "depends_on": [
        "C1"
      ],
      "grounding": "new: no precedent found",
      "id": "C4",
      "kind": "Functional",
      "provisional": "Q1",
      "repo": "web",
      "statement": "The order page offers cancellation for an ACTIVE order."
    }
  ],
  "current_behavior": "Orders change state only through `OrderService` (`src/orders/service.ts`). There is no cancellation path; `OrderService.refund` is the closest precedent and publishes `order.refunded`.",
  "data_impact": [],
  "date": "2026-07-28",
  "deliverables": [
    {
      "areas": [
        "orders",
        "events"
      ],
      "depends_on": [],
      "id": "D1",
      "outcome": "A customer can cancel an ACTIVE order through the API and the warehouse receives `order.cancelled`.",
      "repo": "backend",
      "title": "Cancellation endpoint"
    },
    {
      "areas": [
        "orders-ui"
      ],
      "depends_on": [
        "D1"
      ],
      "id": "D2",
      "outcome": "A customer can cancel an ACTIVE order from the order page.",
      "repo": "web",
      "title": "Cancel button"
    }
  ],
  "evidence": [
    {
      "fact": "`MessageGroupId` is required for every message sent to a FIFO queue.",
      "id": "E1",
      "note": null,
      "rule": "Set the order id as the message group of `order.cancelled`.",
      "source": "https://docs.aws.amazon.com/AWSSimpleQueueService/latest/APIReference/API_SendMessage.html",
      "version": "API version 2012-11-05"
    }
  ],
  "in_scope": [
    {
      "criteria": [
        "C1",
        "C2",
        "C3"
      ],
      "text": "Customer-initiated cancellation of an ACTIVE order."
    },
    {
      "criteria": [
        "C4"
      ],
      "text": "A cancel button on the order page."
    }
  ],
  "notes": [],
  "objective": "Let customers cancel an order while it is still ACTIVE, and tell the warehouse through the existing event queue, so support staff stop editing order rows by hand.",
  "open_questions": [
    {
      "id": "Q1",
      "multi_select": false,
      "options": [
        {
          "description": "Matches the refund flow, which rejects orders in the wrong state.",
          "label": "No, only before shipping"
        },
        {
          "description": "Needs a new RETURN_REQUESTED state and a warehouse notification.",
          "label": "Yes, as a return request"
        }
      ],
      "question": "May a customer cancel an order after it has shipped?",
      "recommended": 0,
      "tag": "Scope"
    }
  ],
  "out_of_scope": [
    {
      "reason": "Q1 is open; the research recommends rejecting it.",
      "text": "Cancelling a shipped order."
    },
    {
      "reason": "The payment service owns refunds and the request does not ask for it.",
      "text": "Refund of the payment."
    }
  ],
  "repos": [
    {
      "key": "backend",
      "root": "/work/shop/backend"
    },
    {
      "key": "web",
      "root": "/work/shop/web"
    }
  ],
  "requirements": [
    {
      "acceptance": [
        {
          "given": "order `o-1` owned by user `u-1` with status ACTIVE",
          "id": "AC1.1",
          "then": "the response status is 200 and `o-1` has status CANCELLED",
          "when": "`u-1` sends `POST /orders/o-1/cancel`"
        }
      ],
      "covers": [
        "C1"
      ],
      "deliverable": "D1",
      "id": "R1",
      "pattern": "event-driven",
      "rests_on": [],
      "statement": "WHEN the owner of an ACTIVE order requests its cancellation THE SYSTEM SHALL set the order status to CANCELLED.",
      "title": "Cancel an active order"
    },
    {
      "acceptance": [
        {
          "given": "order `o-1` owned by user `u-1` with status ACTIVE",
          "id": "AC2.1",
          "then": "the response status is 403 and `o-1` remains ACTIVE",
          "when": "user `u-2` sends `POST /orders/o-1/cancel`"
        }
      ],
      "covers": [
        "C2"
      ],
      "deliverable": "D1",
      "id": "R2",
      "pattern": "unwanted",
      "rests_on": [],
      "statement": "IF the caller does not own the order THEN THE SYSTEM SHALL reject the cancellation with status 403.",
      "title": "Reject another user's order"
    },
    {
      "acceptance": [
        {
          "given": "order `o-1` with status ACTIVE",
          "id": "AC3.1",
          "then": "one `order.cancelled` message with `orderId` `o-1` and message group `o-1` is on the queue",
          "when": "its owner cancels it"
        }
      ],
      "covers": [
        "C3"
      ],
      "deliverable": "D1",
      "id": "R3",
      "pattern": "event-driven",
      "rests_on": [
        "E1"
      ],
      "statement": "WHEN an order is cancelled THE SYSTEM SHALL publish `order.cancelled` carrying the order identifier.",
      "title": "Publish the cancellation"
    },
    {
      "acceptance": [
        {
          "given": "the order page for ACTIVE order `o-1`",
          "id": "AC4.1",
          "then": "a Cancel order button is visible",
          "when": "the page renders"
        },
        {
          "given": "the order page for SHIPPED order `o-2`",
          "id": "AC4.2",
          "then": "no Cancel order button is shown",
          "when": "the page renders"
        }
      ],
      "covers": [
        "C4"
      ],
      "deliverable": "D2",
      "id": "R4",
      "pattern": "state-driven",
      "rests_on": [],
      "statement": "WHILE an order is ACTIVE THE SYSTEM SHALL show a Cancel order button on the order page.",
      "title": "Offer the cancel button"
    }
  ],
  "research": [
    "/work/shop/.ostra/sessions/s_01/backend/ostra-research-20260728-141030-order-lifecycle.md"
  ],
  "title": "Order cancellation"
};

export const planDoc: PlanDoc = {
  "clarifying_questions": [],
  "date": "2026-07-28",
  "deliverables": [
    {
      "id": "D1",
      "title": "Cancellation endpoint"
    },
    {
      "id": "D2",
      "title": "Cancel button"
    }
  ],
  "phases": [
    {
      "areas": [
        "orders",
        "events"
      ],
      "complexity": "Medium",
      "constraints": [
        {
          "fact": "`MessageGroupId` is required for every message sent to a FIFO queue.",
          "id": "E1",
          "note": null,
          "rule": "Set the order id as the message group of `order.cancelled`.",
          "source": "https://docs.aws.amazon.com/AWSSimpleQueueService/latest/APIReference/API_SendMessage.html",
          "version": "API version 2012-11-05"
        }
      ],
      "context": "This is the first phase. No prior phases.",
      "deliverable": "D1",
      "depends_on": [],
      "description": "Adds `OrderService.cancel` and publishes `order.cancelled`.",
      "id": 1,
      "name": "Cancel transition",
      "repo": "backend",
      "repo_root": "/work/shop/backend",
      "requirements": [
        {
          "id": "R1",
          "statement": "WHEN the owner of an ACTIVE order requests its cancellation THE SYSTEM SHALL set the order status to CANCELLED."
        },
        {
          "id": "R2",
          "statement": "IF the caller does not own the order THEN THE SYSTEM SHALL reject the cancellation with status 403."
        },
        {
          "id": "R3",
          "statement": "WHEN an order is cancelled THE SYSTEM SHALL publish `order.cancelled` carrying the order identifier."
        }
      ],
      "skills": [
        "backend-service"
      ],
      "steps": [
        {
          "action": "Add method `cancel(id: string, userId: string): Promise<Order>`. Logic: (1) load by `id`, throw `NotFound` if absent; (2) throw `Forbidden` when `order.userId` differs from `userId`; (3) throw `InvalidState` unless status is ACTIVE; (4) set CANCELLED and save; (5) publish `order.cancelled` with `orderId`.",
          "binding_rules": [
            {
              "id": "E1",
              "rule": "Set the order id as the message group of `order.cancelled`."
            }
          ],
          "change": "Modify",
          "delivers": [
            "R1",
            "R2",
            "R3"
          ],
          "file": "src/orders/service.ts",
          "id": "1.1",
          "read_first": [
            "src/orders/service.ts",
            "src/events/publisher.ts"
          ],
          "size": "Medium",
          "skills": [
            "backend-service"
          ],
          "title": "Add the cancel transition",
          "verify": "npm run build"
        },
        {
          "action": "Add `POST /orders/:id/cancel`, calling `OrderService.cancel` with the session user and mapping `Forbidden` to 403 and `InvalidState` to 409.",
          "binding_rules": [],
          "change": "Modify",
          "delivers": [
            "R1",
            "R2"
          ],
          "file": "src/orders/routes.ts",
          "id": "1.2",
          "read_first": [
            "src/orders/routes.ts"
          ],
          "size": "Small",
          "skills": [
            "backend-service"
          ],
          "title": "Add the cancel route",
          "verify": "npm run build"
        }
      ],
      "test_policy": "Required",
      "test_rationale": "Step 1.1 adds a transition with two rejection branches.",
      "verification": "npm run build"
    },
    {
      "areas": [
        "orders-ui"
      ],
      "complexity": "Low",
      "constraints": [],
      "context": "Phase 1 added `POST /orders/:id/cancel` in the backend repo: no body, 200 with the order JSON, 403 when the caller does not own the order, 409 when it is not ACTIVE.",
      "deliverable": "D2",
      "depends_on": [
        1
      ],
      "description": "Shows Cancel order on ACTIVE orders and calls the endpoint.",
      "id": 2,
      "name": "Cancel button",
      "repo": "web",
      "repo_root": "/work/shop/web",
      "requirements": [
        {
          "id": "R4",
          "statement": "WHILE an order is ACTIVE THE SYSTEM SHALL show a Cancel order button on the order page."
        }
      ],
      "skills": [
        "react-component"
      ],
      "steps": [
        {
          "action": "Render a Cancel order button only when `order.status` is ACTIVE. On click, call `POST /orders/:id/cancel` and replace the order with the response.",
          "binding_rules": [],
          "change": "Modify",
          "delivers": [
            "R4"
          ],
          "file": "src/orders/OrderPage.tsx",
          "id": "2.1",
          "read_first": [
            "src/orders/OrderPage.tsx"
          ],
          "size": "Small",
          "skills": [
            "react-component"
          ],
          "title": "Add the button",
          "verify": "npm run build"
        }
      ],
      "test_policy": "Required",
      "test_rationale": "Step 2.1 adds a status branch that decides whether the button renders.",
      "verification": "npm run build"
    }
  ],
  "pre_checks": [
    {
      "check": "Surviving callers (8A)",
      "result": "Clean",
      "scope": "0 symbols removed, renamed, or moved"
    },
    {
      "check": "Target-module imports (8B)",
      "result": "Clean",
      "scope": "0 files moved between modules"
    }
  ],
  "repos": [
    {
      "key": "backend",
      "root": "/work/shop/backend"
    },
    {
      "key": "web",
      "root": "/work/shop/web"
    }
  ],
  "risks": [
    {
      "impact": "Cancelled orders still ship.",
      "likelihood": "Medium",
      "mitigation": "Coordinate the consumer change before release.",
      "risk": "The warehouse consumer ignores unknown event types."
    }
  ],
  "spec": "/work/shop/.ostra/sessions/s_01/ostra-spec-20260728-141030-order-cancellation.md",
  "stakes": "Medium",
  "stakes_rationale": "Adds a state transition other flows read and a new event type the warehouse consumes.",
  "success_criteria": [
    {
      "id": null,
      "text": "Build passes: `npm run build` in backend and web."
    },
    {
      "id": "AC1.1",
      "text": "Cancelling ACTIVE order `o-1` as its owner returns 200 and sets CANCELLED."
    },
    {
      "id": "AC2.1",
      "text": "Cancelling as another user returns 403 and leaves the order ACTIVE."
    },
    {
      "id": "AC3.1",
      "text": "One `order.cancelled` message with group `o-1` reaches the queue."
    },
    {
      "id": "AC4.1",
      "text": "The order page shows Cancel order for an ACTIVE order."
    },
    {
      "id": "AC4.2",
      "text": "The order page hides Cancel order for a SHIPPED order."
    }
  ],
  "summary": "Adds a cancel transition to the order service and its endpoint, publishes `order.cancelled` on the existing FIFO queue, then adds the Cancel order button to the web order page.",
  "title": "Order cancellation",
  "verification": [
    "**Per-step / per-phase:** the phase's repo's `build` command.",
    "**Final:** each repo's `build` command after its phases."
  ]
};

export function phaseDoc(id: number): PhaseDoc | null {
  const phase = planDoc.phases.find((p) => p.id === id);
  if (!phase) return null;
  const deliverable_title = planDoc.deliverables.find((d) => d.id === phase.deliverable)?.title ?? null;
  return { plan: planDoc.title, date: planDoc.date, spec: planDoc.spec, deliverable_title, phase };
}

/** The typed document behind a mock session-dir file, as `GET /api/artifacts` returns it. */
export function documentFor(path: string): DocumentView | null {
  const name = path.split("/").pop() ?? "";
  if (name.startsWith("ostra-research-")) return { document: { kind: "research", doc: researchDoc }, issues: [] };
  if (name.startsWith("ostra-spec-")) return { document: { kind: "spec", doc: specDoc }, issues: [] };
  if (name.startsWith("ostra-plan-")) {
    const m = /-phase-(\d+)/.exec(name);
    if (!m) return { document: { kind: "plan", doc: planDoc }, issues: [] };
    const doc = phaseDoc(Number(m[1]));
    return doc ? { document: { kind: "phase", doc }, issues: [] } : null;
  }
  return null;
}
