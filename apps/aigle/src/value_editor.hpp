#pragma once
#include <QCoreApplication>
#include <QJsonObject>
#include <QWidget>
#include <functional>

namespace aigle {
// Structured editors for tagged variants, optional groups and collections.
// Validation still belongs to the Rust document layer.
class ValueEditor final : public QWidget {
    Q_DECLARE_TR_FUNCTIONS(aigle::ValueEditor)
  public:
    ValueEditor(QJsonObject schema, QJsonValue value, QJsonObject root, QWidget* parent = nullptr);
    [[nodiscard]] QJsonValue value() const { return read_(); }

  private:
    std::function<QJsonValue()> read_;
};
bool editValue(QWidget* parent, const QString& name, const QJsonObject& schema, QJsonValue& value,
               const QJsonObject& root);
[[nodiscard]] QJsonObject schemaAt(const QJsonObject& root, const QString& path);
} // namespace aigle
