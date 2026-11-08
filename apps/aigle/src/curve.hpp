#pragma once
#include "gui_types.hpp"
#include <QJsonArray>
#include <QJsonObject>
#include <QPointF>
#include <QWidget>
#include <vector>

namespace aigle {
class Curve final : public QWidget {
    Q_OBJECT
  public:
    explicit Curve(const CurvePoints& points, QWidget* parent = nullptr);
    [[nodiscard]] CurvePoints points() const;
    void setHistogram(const Histogram& histogram);
    void setChannel(const QString& channel);
    void setPoints(const CurvePoints& points);
  signals:
    void dragStarted();
    void pointsPreviewed(const CurvePoints& points);
    void pointsChanged(const CurvePoints& points);

  protected:
    void paintEvent(QPaintEvent*) override;
    void mousePressEvent(QMouseEvent* event) override;
    void mouseDoubleClickEvent(QMouseEvent* event) override;
    void mouseMoveEvent(QMouseEvent* event) override;
    void mouseReleaseEvent(QMouseEvent*) override {
        if (selected_ >= 0) {
            if (removed_) points_.erase(points_.begin() + selected_);
            removed_ = false;
            selected_ = -1;
            emit pointsChanged(points());
            update();
        }
    }

  private:
    [[nodiscard]] QRectF plot() const;
    [[nodiscard]] QPointF toPlot(QPointF value) const;
    [[nodiscard]] QPointF fromPlot(QPointF value) const;
    std::vector<QPointF> points_;
    Histogram histogram_;
    QString channel_ = "rgb";
    QPointF grabOffset_;
    bool removed_ = false;
    int selected_ = -1;
};
} // namespace aigle
