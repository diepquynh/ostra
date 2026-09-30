"""Orders stored in SQLite."""

import json
import sqlite3
import threading

from .domain import Line, Order


class OrderRepository:
    def __init__(self, path: str):
        self._db = sqlite3.connect(path, check_same_thread=False)
        self._lock = threading.Lock()
        with self._lock, self._db:
            self._db.execute(
                "CREATE TABLE IF NOT EXISTS orders (id INTEGER PRIMARY KEY, customer TEXT NOT NULL,"
                " lines TEXT NOT NULL, status TEXT NOT NULL)"
            )

    def add(self, order: Order) -> Order:
        lines = json.dumps([[line.sku, line.unit_cents, line.quantity] for line in order.lines])
        with self._lock, self._db:
            cursor = self._db.execute(
                "INSERT INTO orders (customer, lines, status) VALUES (?, ?, ?)",
                (order.customer, lines, order.status),
            )
        order.id = cursor.lastrowid
        return order

    def get(self, order_id: int) -> Order | None:
        with self._lock:
            row = self._db.execute(
                "SELECT id, customer, lines, status FROM orders WHERE id = ?", (order_id,)
            ).fetchone()
        if row is None:
            return None
        lines = [Line(sku, unit_cents, quantity) for sku, unit_cents, quantity in json.loads(row[2])]
        return Order(customer=row[1], lines=lines, status=row[3], id=row[0])

    def set_status(self, order_id: int, status: str) -> None:
        with self._lock, self._db:
            self._db.execute("UPDATE orders SET status = ? WHERE id = ?", (status, order_id))

    def close(self) -> None:
        self._db.close()
