import os
import tempfile
import unittest

from shop.domain import PAID, SHIPPED, InvalidState, Line, NotFound
from shop.repository import OrderRepository
from shop.service import OrderService


class OrderServiceTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.repo = OrderRepository(os.path.join(self.dir.name, "shop.db"))
        self.service = OrderService(self.repo)

    def tearDown(self):
        self.repo.close()
        self.dir.cleanup()

    def test_place_stores_a_placed_order(self):
        order = self.service.place("ada", [Line("pen", 125, 2)])
        self.assertEqual(self.repo.get(order.id).status, "placed")

    def test_place_rejects_an_order_without_lines(self):
        with self.assertRaises(ValueError):
            self.service.place("ada", [])

    def test_pay_then_ship(self):
        order = self.service.place("ada", [Line("pen", 125, 1)])
        self.service.pay(order.id)
        self.assertEqual(self.repo.get(order.id).status, PAID)
        self.service.ship(order.id)
        self.assertEqual(self.repo.get(order.id).status, SHIPPED)

    def test_ship_before_pay_is_refused(self):
        order = self.service.place("ada", [Line("pen", 125, 1)])
        with self.assertRaises(InvalidState):
            self.service.ship(order.id)

    def test_get_of_an_unknown_order_raises_not_found(self):
        with self.assertRaises(NotFound):
            self.service.get(42)
