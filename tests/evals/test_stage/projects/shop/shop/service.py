"""The order rules."""

from .domain import PAID, PLACED, SHIPPED, InvalidState, Line, NotFound, Order
from .repository import OrderRepository


class OrderService:
    def __init__(self, repo: OrderRepository):
        self.repo = repo

    def place(self, customer: str, lines: list[Line]) -> Order:
        if not customer.strip():
            raise ValueError("an order needs a customer")
        if not lines:
            raise ValueError("an order needs at least one line")
        for line in lines:
            if line.quantity <= 0:
                raise ValueError(f"the quantity of {line.sku} must be positive")
        return self.repo.add(Order(customer=customer, lines=lines))

    def get(self, order_id: int) -> Order:
        order = self.repo.get(order_id)
        if order is None:
            raise NotFound(f"no order {order_id}")
        return order

    def pay(self, order_id: int) -> Order:
        return self._move(order_id, PLACED, PAID)

    def ship(self, order_id: int) -> Order:
        return self._move(order_id, PAID, SHIPPED)

    def _move(self, order_id: int, source: str, target: str) -> Order:
        order = self.get(order_id)
        if order.status != source:
            raise InvalidState(f"order {order_id} is {order.status}, not {source}")
        self.repo.set_status(order_id, target)
        order.status = target
        return order
