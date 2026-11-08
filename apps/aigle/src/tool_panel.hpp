#pragma once
#include "mask_types.hpp"
#include "commands.hpp"
#include <QCheckBox>
#include <QComboBox>
#include <QDoubleSpinBox>
#include <QJsonArray>
#include <QJsonObject>
#include <QListWidget>
#include <QVBoxLayout>
#include <QWidget>
#include <functional>
namespace aigle {
class ToolPanel final : public QWidget {
    Q_OBJECT
  public:
    explicit ToolPanel(QWidget* parent = nullptr);
    std::optional<qint64> selectedMaskId() const { const auto masks = params_["masks"].toArray(); return selected_ >= 0 && selected_ < masks.size() ? std::optional<qint64>{masks[selected_].toObject()["id"].toInteger()} : std::nullopt; }
    void selectCreatedMask(qint64 id) { pendingMask_ = id; }
    void setPage(const QString& page);
    void toggleOverlay() { if (overlay_) overlay_->setChecked(!overlay_->isChecked()); }
    void setParams(const QJsonObject& params);
    void setCropAngle(double angle);
    void setGuides(const QList<MaskGuide>& guides);
  signals:
    void command(const aigle::EngineCommand& command);
    void toolSelected(const QString& tool);
    void done();
    void cropOrientationChanged(int operation);
    void horizonRequested();
    void cropGuideChanged(int guide);
    void cropAccepted();
    void cropCancelled();
    void aspectChanged(double ratio);
    void cropAngleChanged(double angle);
    void cropAngleGestureStarted();
    void cropAngleGestureFinished();
    void maskSelected(const aigle::MaskGuide& guide);

  private:
    void rebuild();
    void selectMask(int row);
    void changeMask(const QString& path, QJsonValue value);
    void publishMasks(const QJsonArray& masks, const QString& label);
    QJsonObject params_;
    std::optional<qint64> pendingMask_;
    QList<MaskGuide> guides_;
    QString page_;
    QVBoxLayout* body_;
    QListWidget* masks_ = nullptr;
    QCheckBox *overlay_ = nullptr, *invert_ = nullptr;
    QList<std::function<void()>> sync_;
    int selected_ = 0;
    bool updating_ = false;
    QDoubleSpinBox* cropAngleEditor_ = nullptr;
};
} // namespace aigle
