#include "curve.hpp"
#include "localization.hpp"
#include <QJsonObject>
#include <QMouseEvent>
#include <QPainter>
#include <QPainterPath>
#include <algorithm>
#include <cmath>

namespace aigle {
namespace {
auto curveEvaluator(const std::vector<QPointF>& shown) {
    const auto n = shown.size();
    std::vector<double> d(n - 1), m(n);
    for (size_t i = 0; i + 1 < n; ++i)
        d[i] = (shown[i + 1].y() - shown[i].y()) /
               std::max(1e-9, shown[i + 1].x() - shown[i].x());
    m.front() = d.front();
    m.back() = d.back();
    for (size_t i = 1; i + 1 < n; ++i)
        m[i] = d[i - 1] * d[i] <= 0 ? 0 : (d[i - 1] + d[i]) / 2;
    for (size_t i = 0; i + 1 < n; ++i) {
        if (d[i] == 0)
            m[i] = m[i + 1] = 0;
        else {
            const double s = std::pow(m[i] / d[i], 2) + std::pow(m[i + 1] / d[i], 2);
            if (s > 9) {
                const double t = 3 / std::sqrt(s);
                m[i] *= t;
                m[i + 1] *= t;
            }
        }
    }
    return [shown, m](double x) {
        const auto upper = std::ranges::upper_bound(shown, x, {}, &QPointF::x);
        double y = shown.front().y();
        if (upper == shown.end())
            y = shown.back().y();
        else if (upper != shown.begin()) {
            const auto i = size_t(std::distance(shown.begin(), upper) - 1);
            const auto a = shown[i], b = shown[i + 1];
            const double h = b.x() - a.x(), t = (x - a.x()) / h, t2 = t * t, t3 = t2 * t;
            y = (2 * t3 - 3 * t2 + 1) * a.y() + (t3 - 2 * t2 + t) * h * m[i] +
                (-2 * t3 + 3 * t2) * b.y() + (t3 - t2) * h * m[i + 1];
        }
        return y;
    };
}
} // namespace

Curve::Curve(const CurvePoints& points, QWidget* parent) : QWidget(parent) {
    setMinimumSize(320, 320);
    bindTranslation(this, [this] {
        setAccessibleName(tr("Point curve"));
        setToolTip(tr("Click to add a point on the curve. Drag to adjust. Double-click, right-click, or drag an interior point outside the graph to remove it."));
        setAccessibleDescription(toolTip());
    });
    points_.assign(points.begin(), points.end());
    if (points_.size() < 2)
        points_ = {{0, 0}, {1, 1}};
    std::ranges::sort(points_, {}, &QPointF::x);
}
void Curve::setHistogram(const Histogram& histogram) {
    histogram_ = histogram;
    update();
}
void Curve::setChannel(const QString& channel) {
    channel_ = channel;
    update();
}
CurvePoints Curve::points() const {
    CurvePoints result;
    for (size_t i = 0; i < points_.size(); ++i) {
        if (removed_ && int(i) == selected_) continue;
        const auto point = points_[i];
        result.append(point);
    }
    return result;
}
void Curve::setPoints(const CurvePoints& points) {
    if (selected_ >= 0)
        return;
    points_.clear();
    points_.assign(points.begin(), points.end());
    if (points_.size() < 2)
        points_ = {{0, 0}, {1, 1}};
    std::ranges::sort(points_, {}, &QPointF::x);
    update();
}
QRectF Curve::plot() const {
    const auto side = std::min(width(), height()) - 32;
    return {16, 16, double(side), double(side)};
}
QPointF Curve::toPlot(QPointF p) const {
    const auto r = plot();
    return {r.x() + p.x() * r.width(), r.bottom() - p.y() * r.height()};
}
QPointF Curve::fromPlot(QPointF p) const {
    const auto r = plot();
    return {std::clamp((p.x() - r.x()) / r.width(), 0.0, 1.0),
            std::clamp((r.bottom() - p.y()) / r.height(), 0.0, 1.0)};
}
void Curve::paintEvent(QPaintEvent*) {
    auto shown = points_;
    if (removed_ && selected_ >= 0) shown.erase(shown.begin() + selected_);
    QPainter painter(this);
    painter.fillRect(rect(), palette().base());
    painter.setRenderHint(QPainter::Antialiasing);
    const auto r = plot();
    const auto bins = channel_ == "red" ? histogram_.red : channel_ == "green" ? histogram_.green : channel_ == "blue" ? histogram_.blue : histogram_.luma;
    if (bins.size() >= 2) {
        double peak = 1;
        for (qsizetype i = 1; i + 1 < bins.size(); ++i)
            peak = std::max(peak, bins[i]);
        QPainterPath area;
        area.moveTo(toPlot({0, 0}));
        for (qsizetype i = 0; i < bins.size(); ++i) {
            const double height = std::sqrt(std::clamp(bins[i] / peak, 0.0, 1.0)) * .85;
            area.lineTo(toPlot({double(i) / double(bins.size() - 1), height}));
        }
        area.lineTo(toPlot({1, 0}));
        area.closeSubpath();
        auto color = palette().text().color();
        color.setAlpha(28);
        painter.fillPath(area, color);
    }
    painter.setPen(palette().mid().color());
    for (int i = 0; i <= 4; ++i) {
        const double t = i / 4.0;
        painter.drawLine(toPlot({t, 0}), toPlot({t, 1}));
        painter.drawLine(toPlot({0, t}), toPlot({1, t}));
    }
    const auto evaluate = curveEvaluator(shown);
    QPainterPath line;
    for (int sample = 0; sample <= 256; ++sample) {
        const double x = sample / 256.0;
        const double y = evaluate(x);
        if (sample == 0)
            line.moveTo(toPlot({x, y}));
        else
            line.lineTo(toPlot({x, y}));
    }
    painter.setPen(QPen(palette().highlight().color(), 2));
    painter.drawPath(line);
    painter.setBrush(palette().text());
    for (const auto& point : shown)
        painter.drawEllipse(toPlot(point), 4, 4);
    Q_UNUSED(r);
}
void Curve::mousePressEvent(QMouseEvent* event) {
    removed_ = false;
    selected_ = -1;
    for (size_t i = 0; i < points_.size(); ++i)
        if (QLineF(toPlot(points_[i]), event->position()).length() < 10)
            selected_ = int(i);
    if (event->button() == Qt::RightButton) {
        const bool removable = selected_ > 0 && size_t(selected_) + 1 < points_.size();
        if (removable) points_.erase(points_.begin() + selected_);
        selected_ = -1;
        update();
        if (removable) emit pointsChanged(points());
        return;
    }
    if (event->button() != Qt::LeftButton)
        return;
    if (selected_ < 0) {
        if (!plot().contains(event->position())) return;
        auto point = fromPlot(event->position());
        if (std::ranges::any_of(points_, [point](auto existing) {
                return std::abs(existing.x() - point.x()) < 0.002;
            }))
            return;
        point.setY(curveEvaluator(points_)(point.x()));
        const auto at = std::ranges::upper_bound(points_, point.x(), {}, &QPointF::x);
        selected_ = int(std::distance(points_.begin(), at));
        points_.insert(at, point);
    }
    grabOffset_ = toPlot(points_[size_t(selected_)]) - event->position();
    emit dragStarted();
    emit pointsPreviewed(points());
    update();
}
void Curve::mouseDoubleClickEvent(QMouseEvent* event) {
    if (event->button() != Qt::LeftButton) return;
    removed_ = false;
    selected_ = -1;
    for (size_t i = 1; i + 1 < points_.size(); ++i) {
        if (QLineF(toPlot(points_[i]), event->position()).length() >= 10) continue;
        points_.erase(points_.begin() + int(i));
        emit pointsChanged(points());
        update();
        return;
    }
}
void Curve::mouseMoveEvent(QMouseEvent* event) {
    if (selected_ < 0 || !(event->buttons() & Qt::LeftButton))
        return;
    auto point = fromPlot(event->position() + grabOffset_);
    const auto i = size_t(selected_);
    removed_ = i > 0 && i + 1 < points_.size() &&
        !plot().adjusted(-24, -24, 24, 24).contains(event->position());
    if (i == 0)
        point.setX(0);
    else if (i + 1 == points_.size())
        point.setX(1);
    else
        point.setX(std::clamp(point.x(), points_[i - 1].x() + 0.001, points_[i + 1].x() - 0.001));
    points_[i] = point;
    emit pointsPreviewed(points());
    update();
}
} // namespace aigle
