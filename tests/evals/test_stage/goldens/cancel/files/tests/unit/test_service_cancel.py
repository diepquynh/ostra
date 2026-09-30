import unittest
from unittest.mock import Mock

from shop.domain import CANCELLED, PAID, PLACED, SHIPPED, InvalidState, Line, NotFound, Order
from shop.repository import OrderRepository
from shop.service import OrderService


def service_with(order):
    repo = Mock(spec=OrderRepository)
    repo.get.return_value = order
    return OrderService(repo), repo


def order(status):
    return Order(customer="ada", lines=[Line("pen", 125, 1)], status=status, id=1)


class CancelTest(unittest.TestCase):
    def test_cancels_a_placed_order_and_stores_the_status(self):
        service, repo = service_with(order(PLACED))
        self.assertEqual(service.cancel(1).status, CANCELLED)
        repo.set_status.assert_called_once_with(1, CANCELLED)

    def test_cancels_a_paid_order(self):
        service, repo = service_with(order(PAID))
        self.assertEqual(service.cancel(1).status, CANCELLED)
        repo.set_status.assert_called_once_with(1, CANCELLED)

    def test_refuses_a_shipped_order(self):
        service, repo = service_with(order(SHIPPED))
        with self.assertRaises(InvalidState):
            service.cancel(1)
        repo.set_status.assert_not_called()

    def test_refuses_an_order_that_is_already_cancelled(self):
        service, repo = service_with(order(CANCELLED))
        with self.assertRaises(InvalidState):
            service.cancel(1)
        repo.set_status.assert_not_called()

    def test_an_unknown_order_raises_not_found(self):
        service, _ = service_with(None)
        with self.assertRaises(NotFound):
            service.cancel(1)
