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
      areas: [],
      updated_at: T,
      sections: [
        {
          id: "checkout",
          title: "Checkout",
          purpose: "Turns a cart into a paid order.",
          boundaries: {
            owns: ["Order creation", "The charge call"],
            does_not_own: ["The cart, which the web client owns"],
          },
          assumptions: [
            "Prices in the cart were current when checkout started.",
            "The payment provider is idempotent.",
          ],
          business_flow: [
            { actor: "Customer", action: "Submits the cart", outcome: "A pending order exists" },
            { actor: "API", action: "Charges the card", outcome: "The order is paid" },
          ],
          diagrams: [
            {
              title: "Charge an order",
              kind: "sequence",
              source:
                "sequenceDiagram\n  participant W as Web client\n  participant A as API\n  participant P as Payment provider\n  W->>A: POST /orders\n  A->>P: charge(order, key)\n  P-->>A: charged\n  A-->>W: 201 paid",
            },
          ],
          tables: [
            {
              title: "Order states",
              columns: ["State", "Meaning"],
              rows: [
                ["pending", "Created, not charged yet"],
                ["paid", "Charged"],
              ],
            },
          ],
          concerns: [
            { component: "OrderService", responsibility: "Validates and stores orders" },
            { component: "PaymentClient", responsibility: "Talks to the payment provider" },
          ],
          code_refs: [
            {
              path: "src/order.rs",
              symbol: "OrderService::checkout",
              lines: "40-72",
              note: "The checkout entry point",
            },
          ],
          subsections: [
            {
              id: "checkout-retry",
              title: "Retrying a charge",
              purpose: "Retries a charge that timed out.",
              boundaries: { owns: ["The retry schedule"], does_not_own: [] },
              assumptions: ["A retry reuses the first attempt's idempotency key."],
              business_flow: [],
              diagrams: [],
              tables: [],
              concerns: [],
              code_refs: [{ path: "src/retry.rs", symbol: null, lines: null, note: "The retry job" }],
            },
          ],
        },
      ],
    },
    {
      project: "web",
      overview: "The web client shows the catalog and runs checkout.",
      areas: [],
      updated_at: T,
      sections: [
        {
          id: "cart",
          title: "Cart",
          purpose: "Keeps the items a customer picked until checkout.",
          boundaries: { owns: ["The cart"], does_not_own: ["Prices, which the API owns"] },
          assumptions: ["The cart lives in the browser's storage."],
          business_flow: [],
          diagrams: [],
          tables: [],
          concerns: [],
          code_refs: [{ path: "src/cart.ts", symbol: "useCart", lines: null, note: "The cart hook" }],
          subsections: [],
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
