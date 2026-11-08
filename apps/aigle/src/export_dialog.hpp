#pragma once
#include "export_input.hpp"
#include "commands.hpp"
#include <QCheckBox>
#include <QComboBox>
#include <QDialog>
#include <QImage>
#include <QJsonArray>
#include <QJsonObject>
#include <QLabel>
#include <QLineEdit>
#include <QSpinBox>
#include <QDoubleSpinBox>
#include <QColor>
namespace aigle {
class ExportDialog final : public QDialog {
    Q_OBJECT
  public:
    ExportDialog(const QString& source, const QImage& preview, QSize sourceSize,
                 const QList<ExportFormat>& formats, QWidget* parent = nullptr, QList<ExportInput> inputs = {});
    [[nodiscard]] QList<commands::Export> requests() const;

  private:
    QList<ExportInput> inputs_;
    QList<commands::Export> requestsFor(const QString& name, int sequence, int total) const;
    QString outputStem(const QString& name, int sequence, int total) const;
    void selectFormat(const QString& name);
    void updatePreview();
    void showError(const char* source, const QString& argument = {});
    QByteArray errorSource_;
    QString errorArgument_;
    QJsonObject settings() const;
    void restoreSettings(const QJsonObject& value);
    QSpinBox* longEdge_;
    QLineEdit *artist_, *copyright_, *watermarkImage_;
    QDoubleSpinBox *watermarkSize_, *watermarkOpacity_, *watermarkMargin_;
    QComboBox* watermarkAnchor_;
    QColor watermarkColor_{Qt::white};
    QString source_;
    QSize sourceSize_;
    QList<ExportFormat> formats_;
    QComboBox *format_, *profile_, *size_, *metadata_, *sharpen_, *watermark_;
    QSpinBox* quality_;
    QLineEdit *destination_, *name_, *watermarkText_;
    QCheckBox* paired_;
    QLabel *outputName_, *outputSize_, *outputColor_, *error_;
};
} // namespace aigle
