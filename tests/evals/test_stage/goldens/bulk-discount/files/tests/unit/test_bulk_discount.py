import unittest

from shop.pricing import bulk_discount


class BulkDiscountTest(unittest.TestCase):
    def test_no_discount_below_ten_units(self):
        self.assertEqual(bulk_discount(9, 1000), 0)

    def test_five_percent_from_ten_units(self):
        self.assertEqual(bulk_discount(10, 1000), 500)

    def test_five_percent_up_to_forty_nine_units(self):
        self.assertEqual(bulk_discount(49, 1000), 2450)

    def test_ten_percent_from_fifty_units(self):
        self.assertEqual(bulk_discount(50, 1000), 5000)

    def test_rounds_down_to_whole_cents(self):
        self.assertEqual(bulk_discount(10, 99), 49)

    def test_a_free_item_has_no_discount(self):
        self.assertEqual(bulk_discount(50, 0), 0)

    def test_rejects_a_zero_quantity(self):
        with self.assertRaises(ValueError):
            bulk_discount(0, 1000)

    def test_rejects_a_negative_quantity(self):
        with self.assertRaises(ValueError):
            bulk_discount(-1, 1000)

    def test_rejects_a_negative_unit_price(self):
        with self.assertRaises(ValueError):
            bulk_discount(10, -1)
