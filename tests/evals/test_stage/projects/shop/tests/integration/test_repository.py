import os
import tempfile
import unittest

from shop.domain import PAID, Line, Order
from shop.repository import OrderRepository


class OrderRepositoryTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.repo = OrderRepository(os.path.join(self.dir.name, "shop.db"))

    def tearDown(self):
        self.repo.close()
        self.dir.cleanup()

    def test_add_then_get_returns_the_order(self):
        saved = self.repo.add(Order(customer="ada", lines=[Line("pen", 125, 2)]))
        loaded = self.repo.get(saved.id)
        self.assertEqual(loaded.customer, "ada")
        self.assertEqual(loaded.lines, [Line("pen", 125, 2)])
        self.assertEqual(loaded.status, "placed")

    def test_get_of_an_unknown_id_returns_none(self):
        self.assertIsNone(self.repo.get(99))

    def test_set_status_is_stored(self):
        saved = self.repo.add(Order(customer="ada", lines=[Line("pen", 125, 1)]))
        self.repo.set_status(saved.id, PAID)
        self.assertEqual(self.repo.get(saved.id).status, PAID)
