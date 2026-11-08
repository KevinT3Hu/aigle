#pragma once
#include "gui_types.hpp"
#include "commands.hpp"
#include <QFormLayout>
#include <QJsonObject>
#include <QPointer>
#include <QTabWidget>
#include <QVBoxLayout>
#include <QWidget>
#include <array>
#include <functional>
#include <vector>

namespace aigle {
class Adjustments final : public QWidget {
    Q_OBJECT
  public:
    explicit Adjustments(QWidget* parent = nullptr);
    void showPage(int page);
    void setSchema(const QJsonObject& schema);
    void setParams(const QJsonObject& params);
    void setHistogram(const Histogram& histogram);
    void setImageInfo(const ImageInfo& info);
    void setAuxiliaryPages(QWidget* info, QWidget* metadata);
  signals:
    void command(const aigle::EngineCommand& request);
    void toolSelected(const QString& tool);

  private:
    void updateSourceControls();
    void addFields(QVBoxLayout* layout, const QJsonObject& schema, const QString& prefix);
    void addField(QVBoxLayout* layout, const QString& path, const QString& label, std::function<QString()> translated = {});
    void addExtra(QVBoxLayout* layout, const QString& extra);
    void addStructured(QVBoxLayout* layout, const QString& path, const QString& label, std::function<QString()> translated = {});
    void edit(const QString& path, const QJsonValue& value, bool gesture = false);
    void editJson(const QString& path);
    [[nodiscard]] QJsonValue displayed(const QString& path) const;
    QTabWidget* tabs_;
    std::array<QVBoxLayout*, 3> pages_{};
    QJsonObject params_, schema_;
    ImageInfo info_;
    Histogram histogram_;
    struct Field {
        QString path;
        std::function<void(const QJsonValue&)> update;
    };
    struct Section {
        QPointer<QWidget> body;
        QPointer<QWidget> reset;
        QStringList paths;
    };
    std::vector<Field> fields_;
    std::vector<Section> sections_;
    bool updating_ = false;
    quint64 gesture_ = 0;
};
[[nodiscard]] QJsonValue valueAt(QJsonValue value, const QString& path);
[[nodiscard]] QString title(const QString& text);
} // namespace aigle
