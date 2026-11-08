#include "comparison.hpp"
#include "shortcuts.hpp"
#include <QTest>
class ComparisonTest final : public QObject {
    Q_OBJECT
  private slots:
    void shortcutConflictsIncludePrefixes() {
        QVERIFY(aigle::shortcutConflict(QKeySequence("Ctrl+K"), QKeySequence("Ctrl+K, Ctrl+C")));
        QVERIFY(aigle::shortcutConflict(QKeySequence("Ctrl+K, Ctrl+C"), QKeySequence("Ctrl+K")));
        QVERIFY(aigle::shortcutConflict(QKeySequence("F"), QKeySequence("F")));
        QVERIFY(!aigle::shortcutConflict(QKeySequence(), QKeySequence("F")));
        QVERIFY(!aigle::shortcutConflict(QKeySequence("Ctrl+K, Ctrl+C"),
                                         QKeySequence("Ctrl+K, Ctrl+U")));
    }

    void fitAndLinkedZoom() {
        aigle::ComparisonView first;
        QCOMPARE(first.imageRect({2000, 1000}, {600, 400}), QRectF(0, 50, 600, 300));
        first.zoomAt(2, {300, 200}, {2000, 1000}, {600, 400});
        auto second = first;
        QCOMPARE(first.zoom, second.zoom);
        QCOMPARE(second.imageRect({1000, 2000}, {600, 400}).size(), QSizeF(400, 800));
        QCOMPARE(first.imageRect({2000, 1000}, {600, 400}).center(), QPointF(300, 200));
    }
    void cursorAnchorAndPanBounds() {
        aigle::ComparisonView view;
        view.zoomAt(2, {300, 200}, {2000, 1000}, {600, 400});
        const QPointF anchor(240, 180);
        auto rect = view.imageRect({2000, 1000}, {600, 400});
        const QPointF point((anchor.x() - rect.x()) / rect.width(),
                            (anchor.y() - rect.y()) / rect.height());
        view.zoomAt(1.5, anchor, {2000, 1000}, {600, 400});
        rect = view.imageRect({2000, 1000}, {600, 400});
        QVERIFY(QLineF(anchor, rect.topLeft() +
                                   QPointF(point.x() * rect.width(), point.y() * rect.height()))
                    .length() < 0.001);
        view.pan({100000, 100000}, {2000, 1000}, {600, 400});
        QCOMPARE(view.imageRect({2000, 1000}, {600, 400}).topLeft(), QPointF(0, 0));
        view.zoomAt(.001, anchor, {2000, 1000}, {600, 400});
        QCOMPARE(view.zoom, 1.0);
        QCOMPARE(view.center, QPointF(.5, .5));
    }
};
QTEST_GUILESS_MAIN(ComparisonTest)
#include "comparison_test.moc"
