#pragma once
#include <QIcon>
#include <QString>
class QApplication;
class QToolButton;
class QWidget;
namespace aigle {
void applyTheme(QApplication& app);
[[nodiscard]] QIcon glyph(const QString& name);
QToolButton* iconButton(const QString& name, const QString& label, QWidget* parent = nullptr);
QWidget* section(const QString& title, QWidget* body, bool expanded, QWidget* parent = nullptr);
} // namespace aigle
