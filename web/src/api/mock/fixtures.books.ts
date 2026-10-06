import type { Book } from "../gen/Book";
import type { BookSummary } from "../gen/BookSummary";

const T = "2026-09-30T09:12:00Z";

export const MOCK_BOOK: Book = {
  id: "api_web",
  title: "Documentation for api, web",
  projects: ["api", "web"],
  updated_at: T,
  sessions: ["s-docs-1"],
  glossary: [
    {
      term: "Order",
      definition: "A customer's request to buy one or more items, paid once.",
      code_ref: "api/src/order.rs",
    },
    { term: "Refund", definition: "Money returned to the customer for a paid order.", code_ref: null },
  ],
  architecture: {
    overview: "The web client calls the API over HTTPS. The API owns orders and payments and stores them in Postgres.",
    diagram: {
      title: "Components and links",
      kind: "flowchart",
      source:
        "flowchart LR\n  web[Web client] -->|HTTPS JSON| api[API]\n  api -->|SQL| db[(Postgres)]\n  api -->|HTTPS| psp[Payment provider]",
    },
    components: [
      {
        name: "Web client",
        project: "web",
        role: "Shows the catalog and checkout.",
        owns: ["The cart until checkout"],
      },
      { name: "API", project: "api", role: "Takes orders and payments.", owns: ["Orders", "Payments", "Refunds"] },
      { name: "Payment provider", project: "", role: "Charges cards.", owns: ["Card data"] },
    ],
    links: [
      { from: "Web client", to: "API", protocol: "HTTPS JSON", mode: "sync", payload: "Orders and checkout requests" },
      { from: "API", to: "Payment provider", protocol: "HTTPS", mode: "sync", payload: "Charges and refunds" },
    ],
    failure_recovery: [
      {
        failure: "The payment provider times out",
        detection: "The charge call returns no answer within 10 s",
        recovery: "The order stays pending and a job retries the charge with the same idempotency key",
      },
    ],
    scalability: [
      {
        component: "API",
        scales_by: "More stateless instances behind the load balancer",
        limit: "Postgres connections",
      },
    ],
  },
  parts: [
    {
      project: "api",
      overview: "The API takes orders, charges them, and issues refunds.",
      updated_at: T,
      sections: [
        {
          id: "checkout",
          title: "Checkout",
          summary: "Turns a cart into a paid order.",
          body: "Checkout turns the cart into an order and charges it once. The API owns order creation and the charge call. The web client owns the cart.\n\n### Assumptions\n\n- Prices in the cart were current when checkout started.\n- The payment provider is idempotent.\n\n### Charge an order\n\n```mermaid\nsequenceDiagram\n  participant W as Web client\n  participant A as API\n  participant P as Payment provider\n  W->>A: POST /orders\n  A->>P: charge(order, key)\n  P-->>A: charged\n  A-->>W: 201 paid\n```\n\n### Order states\n\n| State | Meaning |\n| --- | --- |\n| pending | Created, not charged yet |\n| paid | Charged |\n\n## Retrying a charge\n\nA job retries a charge that timed out. A retry reuses the idempotency key of the first try, in `src/retry.rs`.",
          code_refs: [
            {
              path: "src/order.rs",
              symbol: "OrderService::checkout",
              lines: "40-72",
              note: "The checkout entry point",
            },
          ],
        },
      ],
    },
    {
      project: "web",
      overview: "The web client shows the catalog and runs checkout.",
      updated_at: T,
      sections: [
        {
          id: "cart",
          title: "Cart",
          summary: "Keeps the items a customer picked until checkout.",
          body: "The cart lives in the storage of the browser. The API owns the prices.",
          code_refs: [{ path: "src/cart.ts", symbol: "useCart", lines: null, note: "The cart hook" }],
        },
      ],
    },
  ],
};

export const summaryOf = (b: Book): BookSummary => ({
  id: b.id,
  title: b.title,
  projects: b.projects,
  updated_at: b.updated_at,
  sections: b.parts.reduce((n, p) => n + p.sections.length, 0),
  has_architecture: b.architecture != null,
});
