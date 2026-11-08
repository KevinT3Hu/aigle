#pragma once
#include <QApplication>
#include <QDialog>
#include <QDialogButtonBox>
#include <QKeyEvent>
#include <QKeySequenceEdit>
#include <QPushButton>
#include <QSettings>
#include <QShortcut>
namespace aigle {
inline QKeySequence commandShortcut(const QString& id, const QString& fallback) {
    const auto active = qApp->property("aigleEffectiveShortcuts").toMap();
    return QKeySequence::fromString(active.contains(id) ? active.value(id).toString()
        : QSettings().value("shortcuts/" + id, fallback).toString(), QKeySequence::PortableText);
}
class DialogBindings final : public QObject {
    QDialog* dialog_;
    QKeySequence accept_, cancel_;
  public:
    explicit DialogBindings(QDialog* dialog) : QObject(dialog), dialog_(dialog),
        accept_(commandShortcut("dialog_accept", "Return")), cancel_(commandShortcut("dialog_cancel", "Esc")) {
        if (accept_ != QKeySequence("Return") && !accept_.isEmpty()) {
            auto* shortcut = new QShortcut(accept_, dialog);
            connect(shortcut, &QShortcut::activated, this, [this] { acceptDefault(); });
        }
        if (cancel_ != QKeySequence("Esc") && !cancel_.isEmpty()) {
            auto* shortcut = new QShortcut(cancel_, dialog);
            connect(shortcut, &QShortcut::activated, dialog, &QDialog::reject);
        }
        qApp->installEventFilter(this);
    }
    void acceptDefault() {
        for (auto* button : dialog_->findChildren<QPushButton*>())
            if (button->isDefault() && button->isVisible() && button->isEnabled()) { button->click(); return; }
        for (auto* box : dialog_->findChildren<QDialogButtonBox*>())
            for (auto* button : box->buttons())
                if (box->buttonRole(button) == QDialogButtonBox::AcceptRole && button->isEnabled() && button->isVisible()) { button->click(); return; }
    }
  protected:
    bool eventFilter(QObject* watched, QEvent* event) override {
        if (event->type() != QEvent::KeyPress) return false;
        auto* widget = qobject_cast<QWidget*>(watched);
        if (!widget || widget->window() != dialog_) return false;
        for (auto* parent = widget; parent; parent = parent->parentWidget())
            if (qobject_cast<QKeySequenceEdit*>(parent)) return false;
        auto* key = static_cast<QKeyEvent*>(event);
        if (key->modifiers() != Qt::NoModifier) return false;
        if (key->key() == Qt::Key_Escape && cancel_ != QKeySequence("Esc")) return true;
        // Preserve newline input; only suppress the dialog's implicit acceptance.
        if ((key->key() == Qt::Key_Return || key->key() == Qt::Key_Enter) &&
            accept_ != QKeySequence("Return") && (watched == dialog_ || qobject_cast<QPushButton*>(watched))) return true;
        return false;
    }
};
inline void installDialogShortcuts(QDialog* dialog) {
    if (dialog->property("aigleDialogBindings").toBool()) return;
    dialog->setProperty("aigleDialogBindings", true);
    new DialogBindings(dialog);
}
} // namespace aigle
