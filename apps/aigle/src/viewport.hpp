#pragma once
#include <QPointF>
#include <QRectF>
#include <QSizeF>
#include <algorithm>
#include <cmath>

namespace aigle {
// Pure geometry in physical pixels and Crop-space coordinates. A zero scale
// means fit. No foreign calls are needed while navigating the viewport.
class Viewport final {
  public:
    QSizeF image{1, 1};
    QSizeF output{1, 1};
    [[nodiscard]] double fitScale() const noexcept {
        return std::min(output.width() / image.width(), output.height() / image.height());
    }
    [[nodiscard]] double scale() const noexcept { return scale_ > 0 ? scale_ : fitScale(); }
    [[nodiscard]] QRectF region() const noexcept {
        const auto size = output / scale();
        const auto center = scale_ > 0 ? center_ : QPointF(image.width() / 2, image.height() / 2);
        const auto place = [](double c, double extent, double visible) {
            return visible >= extent ? (extent - visible) / 2
                                     : std::clamp(c - visible / 2, 0.0, extent - visible);
        };
        return {QPointF(place(center.x(), image.width(), size.width()),
                        place(center.y(), image.height(), size.height())),
                size};
    }
    [[nodiscard]] QRectF rotationSamplingRegion(double degrees) const noexcept {
        const auto visible = region();
        if (!std::isfinite(degrees) || std::abs(degrees) < 1e-9) return visible;
        const double radians = degrees * 3.141592653589793 / 180;
        const double c = std::cos(radians), s = std::sin(radians);
        const QPointF center(image.width() / 2, image.height() / 2);
        auto bounds = visible; // Keep the unrotated before view covered too.
        for (const auto point : {visible.topLeft(), visible.topRight(),
                                 visible.bottomLeft(), visible.bottomRight()}) {
            const auto d = point - center;
            const auto sample = center + QPointF(c * d.x() + s * d.y(), -s * d.x() + c * d.y());
            bounds.setLeft(std::min(bounds.left(), sample.x()));
            bounds.setTop(std::min(bounds.top(), sample.y()));
            bounds.setRight(std::max(bounds.right(), sample.x()));
            bounds.setBottom(std::max(bounds.bottom(), sample.y()));
        }
        return bounds;
    }
    struct Navigation {
        double zoom = 1;
        QPointF center{0.5, 0.5};
    };
    [[nodiscard]] Navigation navigation() const noexcept {
        const auto c = region().center();
        return {scale() / fitScale(), {c.x() / image.width(), c.y() / image.height()}};
    }
    void setNavigation(const Navigation& value) noexcept {
        if (!std::isfinite(value.zoom) || value.zoom <= 1.0001 ||
            !std::isfinite(value.center.x()) || !std::isfinite(value.center.y())) {
            fit();
            return;
        }
        scale_ = std::min(value.zoom * fitScale(), 16.0);
        center_ = {std::clamp(value.center.x(), 0.0, 1.0) * image.width(),
                   std::clamp(value.center.y(), 0.0, 1.0) * image.height()};
        center_ = region().center();
    }
    void fit() noexcept { scale_ = 0; }
    void zoom(double requested, QPointF anchor) noexcept {
        if (!std::isfinite(requested) || requested <= fitScale() * 1.0001) {
            fit();
            return;
        }
        const auto point = region().topLeft() + anchor / scale();
        scale_ = std::min(requested, 16.0);
        center_ = point - anchor / scale_ + QPointF(output.width(), output.height()) / (2 * scale_);
        center_ = region().center();
    }
    void pan(QPointF delta) noexcept {
        if (scale_ == 0)
            return;
        center_ = region().center() - delta / scale();
        center_ = region().center();
    }

  private:
    double scale_ = 0;
    QPointF center_;
};
} // namespace aigle
