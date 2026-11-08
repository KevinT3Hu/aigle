#include "viewport.hpp"
#include <QTest>
class ViewportTest final : public QObject {
    Q_OBJECT
  private slots:
    void linkedNavigationUsesRelativeMagnificationAndCenter() {
        aigle::Viewport source, target;
        source.image = {6000, 4000}; source.output = {1200, 800};
        target.image = {3000, 2000}; target.output = {600, 400};
        source.zoom(.8, {600, 400});
        source.pan({-120, 80});
        const auto nav = source.navigation();
        target.setNavigation(nav);
        QCOMPARE(target.navigation().zoom, nav.zoom);
        QCOMPARE(target.navigation().center, nav.center);
        QCOMPARE(target.region(), QRectF(source.region().topLeft() / 2, source.region().size() / 2));
        // Different aspect ratios clamp independently without losing fit semantics.
        target.image = {2000, 3000};
        target.setNavigation({});
        QCOMPARE(target.scale(), target.fitScale());
        QCOMPARE(target.navigation().center, QPointF(.5, .5));
        target.setNavigation({4, {100, -100}});
        QVERIFY(target.region().left() >= 0);
        QVERIFY(target.region().top() >= 0);
        QVERIFY(target.region().right() <= target.image.width());
        QVERIFY(target.region().bottom() <= target.image.height());
    }
    void rotationRequestsPixelsOutsideZoomedViewport() {
        aigle::Viewport v;
        v.image = {6000, 4000}; v.output = {1200, 800};
        v.setNavigation({4, {.7, .4}});
        const auto visible = v.region();
        QCOMPARE(v.rotationSamplingRegion(0), visible);
        const auto sampled = v.rotationSamplingRegion(90);
        QVERIFY(sampled.contains(visible));
        const QPointF center(3000, 2000);
        for (const auto corner : {visible.topLeft(), visible.topRight(),
                                  visible.bottomLeft(), visible.bottomRight()}) {
            const auto d = corner - center;
            const auto expected = center + QPointF(d.y(), -d.x());
            QVERIFY(sampled.adjusted(-1e-6, -1e-6, 1e-6, 1e-6).contains(expected));
        }
        QVERIFY(sampled.width() > visible.width());
    }
    void fitCentersImage() {
        aigle::Viewport v;
        v.image = {6000, 4000};
        v.output = {1200, 1000};
        QCOMPARE(v.scale(), 0.2);
        QCOMPARE(v.region(), QRectF(0, -500, 6000, 5000));
    }
    void zoomPreservesCursorPoint() {
        aigle::Viewport v;
        v.image = {6000, 4000};
        v.output = {1200, 800};
        const QPointF cursor(480, 320);
        const auto before = v.region().topLeft() + cursor / v.scale();
        v.zoom(1, cursor);
        QCOMPARE(v.region().topLeft() + cursor / v.scale(), before);
        v.pan({100000, 100000});
        QCOMPARE(v.region().topLeft(), QPointF(0, 0));
        v.zoom(0.01, cursor);
        QCOMPARE(v.scale(), v.fitScale());
    }
};
QTEST_GUILESS_MAIN(ViewportTest)
#include "viewport_test.moc"
