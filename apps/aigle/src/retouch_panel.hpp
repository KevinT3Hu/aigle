#pragma once
#include "retouch_types.hpp"
#include "commands.hpp"
#include "gui_types.hpp"
#include <QComboBox>
#include "local_shortcuts.hpp"
#include <QDoubleSpinBox>
#include <QJsonArray>
#include <QJsonObject>
#include <QLabel>
#include <QListWidget>
#include <QSlider>
#include <QTimer>
#include <QWidget>
namespace aigle {
class RetouchPanel final : public QWidget {
    Q_OBJECT
  public:
    explicit RetouchPanel(QWidget* parent = nullptr);
    void setState(const RetouchParameters& params, quint64 document, quint64 revision,
                  const QList<RetouchGuide>& guides);
    void select(const QString& kind, int index);
    void setResult(const RetouchResult& result);
    void setError(const QString& error);
    void reset();
    void beginNew(const QString& tool);
    void setRadius(double radius);
    RetouchDefaults defaults() const;
  signals:
    void command(const aigle::EngineCommand& request);
    void selected(const QString& kind, int index);
    void defaultsChanged(const aigle::RetouchDefaults& defaults);
    void done();

  protected:
    bool eventFilter(QObject*, QEvent*) override;

  private:
    LocalShortcuts shortcuts_;
    struct Control {
        QString field;
        double factor;
        QWidget* row;
        QDoubleSpinBox* number;
        QSlider* slider;
    };
    void syncControls(const RetouchCorrection& value = {});
    void edit(const QString& field, QJsonValue value, bool preview = false, bool finish = false);
    void cancel();
    void remove(bool all);
    commands::Retouch request(const QString& action) const;
    void newCorrection();
    QListWidget* list_;
    QComboBox *createMode_, *spotMode_;
    QLabel *status_, *selection_;
    QList<Control> controls_;
    RetouchParameters params_;
    RetouchCorrection expected_;
    RetouchDefaults defaultValues_;
    QList<RetouchGuide> guides_;
    QString kind_, session_;
    int index_ = -1;
    quint64 document_ = 0, revision_ = 0;
    bool updating_ = false, gesture_ = false;
    QTimer timer_;
    std::optional<commands::Retouch> pending_;
};
} // namespace aigle
