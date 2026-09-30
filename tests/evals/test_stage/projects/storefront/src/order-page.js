// Renders one order from the shop API (GET /orders/<id>).

export async function loadOrder(apiBase, id) {
  const response = await fetch(`${apiBase}/orders/${id}`);
  if (!response.ok) {
    throw new Error(`order ${id}: ${response.status}`);
  }
  return response.json();
}

export function orderSummary(order) {
  return `${order.customer}: ${order.lines.length} items, ${order.total} (${order.status})`;
}
