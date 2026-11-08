#pragma once
#include "gui_types.hpp"
#include "commands.hpp"
#include <QCoreApplication>
#include <QCheckBox>
#include <QComboBox>
#include <QImage>
#include <QJsonArray>
#include <QJsonObject>
#include <QLabel>
#include <QTimer>
#include <QWidget>
#include <array>
#include <functional>

class QToolButton;
namespace aigle {
class ScopeGraph final : public QWidget {
    Q_DECLARE_TR_FUNCTIONS(aigle::ScopeGraph)
  public:
    explicit ScopeGraph(QWidget* parent = nullptr);
    void setData(const ScopeResult& reply);
    void configure(int mode, bool logarithmic, std::array<bool, 3> channels);
    QRectF plotRect() const;
    std::function<void(const QString&)> hovered;

  protected:
    bool event(QEvent*) override;
    void keyPressEvent(QKeyEvent*) override;
    void paintEvent(QPaintEvent*) override;
    void mouseMoveEvent(QMouseEvent*) override;
    void leaveEvent(QEvent*) override;

  private:
    void rebuildDensity();
    QString readout(const QPointF& point) const;
    ScopeData data_;
    QList<QPointF> targets_;
    std::array<QImage, 5> densities_;
    std::array<bool, 3> channels_{true, true, true};
    QPointF hover_;
    bool hovering_ = false, logarithmic_ = false;
    int mode_ = 0;
    double white_ = 0;
};

class ScopeThumbnail final : public QWidget {
    Q_DECLARE_TR_FUNCTIONS(aigle::ScopeThumbnail)
  public:
    explicit ScopeThumbnail(QWidget* parent = nullptr);
    void setSample(const QImage& image);
    void wholeCrop();
    QRectF imageRect() const;
    QRect selection() const { return selection_; }
    std::function<void(const QRect&)> selected;

  protected:
    bool event(QEvent*) override;
    void paintEvent(QPaintEvent*) override;
    void mousePressEvent(QMouseEvent*) override;
    void mouseMoveEvent(QMouseEvent*) override;
    void mouseReleaseEvent(QMouseEvent*) override;
    void keyPressEvent(QKeyEvent*) override;
    void focusOutEvent(QFocusEvent*) override;

  private:
    QPoint cell(const QPointF& point) const;
    void drag(const QPointF& point);
    void cancelDrag();
    QImage sample_;
    QRect selection_, beforeDrag_;
    QPoint anchor_;
    bool dragging_ = false;
};

class Scopes final : public QWidget {
    Q_OBJECT
  public:
    explicit Scopes(QWidget* parent = nullptr);
    void histogramChanged(const Histogram& histogram);
    void setResult(const ScopeResult& reply);
    void reset();
  signals:
    void command(const aigle::EngineCommand& request);

  private:
    void schedule();
    void configure();
    void savePreferences();
    void renderStatistics();
    ScopeData statisticsData_;
    QString statisticsError_;
    enum class StatisticsState { Waiting, Updating, Error, Ready };
    StatisticsState statisticsState_ = StatisticsState::Waiting;
    ScopeGraph* graph_;
    ScopeThumbnail* thumbnail_;
    QComboBox *mode_, *target_;
    QCheckBox *log_, *region_;
    std::array<QToolButton*, 3> channels_;
    QLabel *statistics_, *readout_;
    QTimer timer_;
    QString inFlight_;
    QRect roi_;
    QSize sampleSize_;
    bool dirty_ = false, available_ = false;
};
} // namespace aigle
