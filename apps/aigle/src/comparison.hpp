#pragma once
#include "gui_types.hpp"
#include <QCoreApplication>
#include <QDialog>
#include <QImage>
#include <QJsonArray>
#include <QJsonObject>
#include <QPointF>
#include <QWidget>
#include <algorithm>
#include <cmath>
#include <functional>
namespace aigle {
struct ComparisonView {
    double zoom = 1;
    QPointF center{0.5, 0.5};
    QRectF imageRect(QSizeF image, QSizeF output) const {
        if (image.isEmpty() || output.isEmpty())
            return {};
        const double scale =
            std::min(output.width() / image.width(), output.height() / image.height()) * zoom;
        const QSizeF size = image * scale;
        const auto offset = [](double c, double extent, double viewport) {
            return extent <= viewport
                       ? (viewport - extent) / 2
                       : std::clamp(viewport / 2 - c * extent, viewport - extent, 0.0);
        };
        return {QPointF(offset(center.x(), size.width(), output.width()),
                        offset(center.y(), size.height(), output.height())),
                size};
    }
    void zoomAt(double factor, QPointF anchor, QSizeF image, QSizeF output) {
        const auto before = imageRect(image, output);
        if (before.isEmpty())
            return;
        const QPointF point((anchor.x() - before.x()) / before.width(),
                            (anchor.y() - before.y()) / before.height());
        zoom = std::clamp(zoom * factor, 1.0, 128.0);
        const auto after = imageRect(image, output);
        center = {point.x() + (output.width() / 2 - anchor.x()) / after.width(),
                  point.y() + (output.height() / 2 - anchor.y()) / after.height()};
        center = {std::clamp(center.x(), 0.0, 1.0), std::clamp(center.y(), 0.0, 1.0)};
        if (zoom == 1)
            center = {0.5, 0.5};
    }
    void pan(QPointF delta, QSizeF image, QSizeF output) {
        const auto rect = imageRect(image, output);
        if (rect.isEmpty() || zoom == 1)
            return;
        center = {
            std::clamp((output.width() / 2 - rect.x() - delta.x()) / rect.width(), 0.0, 1.0),
            std::clamp((output.height() / 2 - rect.y() - delta.y()) / rect.height(), 0.0, 1.0)};
    }
};
class ComparisonPane final : public QWidget {
    Q_DECLARE_TR_FUNCTIONS(aigle::ComparisonPane)
  public:
    explicit ComparisonPane(QWidget* parent = nullptr);
    QImage image;
    QString message;
    QSizeF extent;
    QRectF frameRegion;
    QSizeF imageSize() const { return extent.isEmpty() ? QSizeF(image.size()) : extent; }
    ComparisonView view;
    std::function<void(ComparisonPane*)> navigated;
    void setView(const ComparisonView& value) {
        view = value;
        update();
    }

  protected:
    void paintEvent(QPaintEvent*) override;
    void keyPressEvent(QKeyEvent*) override;
    void wheelEvent(QWheelEvent*) override;
    void mousePressEvent(QMouseEvent*) override;
    void mouseMoveEvent(QMouseEvent*) override;
    void mouseReleaseEvent(QMouseEvent*) override;
    void mouseDoubleClickEvent(QMouseEvent*) override;

  private:
    QPointF last_;
};
class Engine;
class Canvas;
void showComparison(QWidget* parent, Engine* engine, const QList<PhotoIdentity>& paths,
                    std::function<void(const PhotoIdentity&, const QString&)> flag, Canvas* sourceCanvas = nullptr);
} // namespace aigle
