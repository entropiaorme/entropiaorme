"""Tests for the EventBus subscriber and tap contracts.

The tap is the supported full-stream seam: subscription is per-topic by
design, so harness observers that need every publish (the replay
fingerprint recorder, the test-mode event sink) attach as taps rather
than monkeypatching ``publish``. These tests pin the contract those
observers rely on: every publish is observed in order regardless of
topic, taps run before subscribers on the publisher's thread, and a
failing tap can break neither dispatch nor its sibling taps.
"""

from backend.core.event_bus import EventBus


class TestSubscribers:
    def test_publish_dispatches_to_topic_subscribers_only(self):
        bus = EventBus()
        seen: list[tuple[str, object]] = []
        bus.subscribe("a", lambda d: seen.append(("a", d)))
        bus.subscribe("b", lambda d: seen.append(("b", d)))

        bus.publish("a", {"n": 1})

        assert seen == [("a", {"n": 1})]

    def test_subscribe_is_idempotent(self):
        bus = EventBus()
        seen: list[object] = []
        bus.subscribe("a", seen.append)
        bus.subscribe("a", seen.append)

        bus.publish("a", 1)

        assert seen == [1]

    def test_unsubscribe_removes_callback_and_clears_topic(self):
        bus = EventBus()
        seen: list[object] = []
        bus.subscribe("a", seen.append)
        assert bus.has_subscribers("a")

        bus.unsubscribe("a", seen.append)

        assert not bus.has_subscribers("a")
        bus.publish("a", 1)
        assert seen == []

    def test_unsubscribe_unknown_callback_is_noop(self):
        bus = EventBus()
        bus.unsubscribe("a", lambda d: None)
        bus.subscribe("a", lambda d: None)
        bus.unsubscribe("a", lambda d: None)
        assert bus.has_subscribers("a")

    def test_subscriber_exception_does_not_break_dispatch(self):
        bus = EventBus()
        seen: list[object] = []

        def _boom(_data):
            raise RuntimeError("subscriber failure")

        bus.subscribe("a", _boom)
        bus.subscribe("a", seen.append)

        bus.publish("a", 1)

        assert seen == [1]


class TestTaps:
    def test_tap_sees_every_publish_in_order_regardless_of_topic(self):
        bus = EventBus()
        seen: list[tuple[str, object]] = []
        bus.add_tap(lambda topic, data: seen.append((topic, data)))
        # No subscriber on either topic: the tap must still observe both.
        bus.publish("a", {"n": 1})
        bus.publish("b", {"n": 2})

        assert seen == [("a", {"n": 1}), ("b", {"n": 2})]

    def test_tap_sees_the_payload_object_unchanged(self):
        bus = EventBus()
        payload = {"n": 1}
        seen: list[object] = []
        bus.add_tap(lambda _topic, data: seen.append(data))

        bus.publish("a", payload)

        assert seen[0] is payload

    def test_tap_runs_before_subscribers(self):
        bus = EventBus()
        order: list[str] = []
        bus.subscribe("a", lambda _d: order.append("subscriber"))
        bus.add_tap(lambda _topic, _data: order.append("tap"))

        bus.publish("a", 1)

        assert order == ["tap", "subscriber"]

    def test_tap_exception_breaks_neither_dispatch_nor_sibling_taps(self):
        bus = EventBus()
        seen: list[str] = []

        def _boom(_topic, _data):
            raise RuntimeError("tap failure")

        bus.add_tap(_boom)
        bus.add_tap(lambda _topic, _data: seen.append("tap2"))
        bus.subscribe("a", lambda _d: seen.append("subscriber"))

        bus.publish("a", 1)

        assert seen == ["tap2", "subscriber"]

    def test_add_tap_is_idempotent(self):
        bus = EventBus()
        seen: list[str] = []

        def _tap(topic, _data):
            seen.append(topic)

        bus.add_tap(_tap)
        bus.add_tap(_tap)

        bus.publish("a", 1)

        assert seen == ["a"]

    def test_taps_compose_in_install_order(self):
        bus = EventBus()
        order: list[str] = []
        bus.add_tap(lambda _topic, _data: order.append("first"))
        bus.add_tap(lambda _topic, _data: order.append("second"))

        bus.publish("a", 1)

        assert order == ["first", "second"]

    def test_remove_tap_stops_observation(self):
        bus = EventBus()
        seen: list[str] = []

        def _tap(topic, _data):
            seen.append(topic)

        bus.add_tap(_tap)
        bus.publish("a", 1)
        bus.remove_tap(_tap)
        bus.publish("b", 2)

        assert seen == ["a"]

    def test_remove_unknown_tap_is_noop(self):
        bus = EventBus()
        bus.remove_tap(lambda _topic, _data: None)
        bus.publish("a", 1)
