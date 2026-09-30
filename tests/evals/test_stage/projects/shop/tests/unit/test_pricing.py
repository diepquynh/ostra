import unittest

from shop.pricing import line_total


class LineTotalTest(unittest.TestCase):
    def test_multiplies_the_unit_price_by_the_quantity(self):
        self.assertEqual(line_total(250, 4), 1000)

    def test_rejects_a_zero_quantity(self):
        with self.assertRaises(ValueError):
            line_total(250, 0)
