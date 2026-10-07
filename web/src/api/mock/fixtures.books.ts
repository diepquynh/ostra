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
  parts: [
    {
      project: "api",
      overview: "The API takes orders, charges them, and issues refunds.",
      updated_at: T,
      inventory: [],
      sections: [
        {
          id: "checkout",
          title: "Checkout",
          group: "How it works",
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
      inventory: [],
      sections: [
        {
          id: "cart",
          title: "Cart",
          group: "How it works",
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
});
