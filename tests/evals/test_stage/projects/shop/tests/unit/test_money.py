import unittest

from shop.money import to_cents


class ToCentsTest(unittest.TestCase):
    def test_dollars_and_cents(self):
        self.assertEqual(to_cents("12.34"), 1234)

    def test_one_decimal_digit_is_tens_of_cents(self):
        self.assertEqual(to_cents("12.3"), 1230)

    def test_whole_dollars(self):
        self.assertEqual(to_cents("12"), 1200)

    def test_negative_amount(self):
        self.assertEqual(to_cents("-0.50"), -50)

    def test_rejects_three_decimal_digits(self):
        with self.assertRaises(ValueError):
            to_cents("1.234")

    def test_rejects_text(self):
        with self.assertRaises(ValueError):
            to_cents("ten")
