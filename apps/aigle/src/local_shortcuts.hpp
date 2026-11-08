#pragma once
#include "dialog_shortcuts.hpp"
#include <QElapsedTimer>
#include <functional>

namespace aigle {
// QWindow tools do not have QWidget shortcut contexts. Match their bindings here,
// including chords, before a parent window can consume the same keystrokes.
class LocalShortcuts {
    QList<int> prefix_;
    QElapsedTimer elapsed_;
  public:
    using Binding = QPair<QString, QString>;
    void reset() { prefix_.clear(); }
    bool handle(QEvent* event, const QList<Binding>& bindings,
                const std::function<void(const QString&)>& execute) {
        if (event->type() == QEvent::FocusOut || event->type() == QEvent::Hide ||
            event->type() == QEvent::WindowDeactivate) reset();
        if (event->type() != QEvent::ShortcutOverride && event->type() != QEvent::KeyPress) return false;
        if (elapsed_.isValid() && elapsed_.elapsed() > 1500) reset();
        auto* key = static_cast<QKeyEvent*>(event);
        if (key->key() == Qt::Key_Control || key->key() == Qt::Key_Shift ||
            key->key() == Qt::Key_Alt || key->key() == Qt::Key_Meta) return false;
        auto keys = prefix_;
        const int code = key->key() == Qt::Key_Enter ? Qt::Key_Return : key->key();
        keys.append(QKeyCombination(key->modifiers(), Qt::Key(code)).toCombined());
        if (keys.size() > 4) { reset(); return false; }
        const QKeySequence sequence(QKeyCombination::fromCombined(keys.value(0)),
            QKeyCombination::fromCombined(keys.value(1)), QKeyCombination::fromCombined(keys.value(2)),
            QKeyCombination::fromCombined(keys.value(3)));
        QString exact;
        bool partial = false;
        for (const auto& [id, fallback] : bindings) {
            const auto binding = commandShortcut(id, fallback);
            if (binding.isEmpty()) continue;
            const auto match = sequence.matches(binding);
            if (match == QKeySequence::ExactMatch) exact = id;
            partial |= match == QKeySequence::PartialMatch;
        }
        if (!exact.isEmpty() || partial) {
            event->accept();
            if (event->type() == QEvent::KeyPress && !key->isAutoRepeat()) {
                if (!exact.isEmpty()) { reset(); execute(exact); }
                else { prefix_ = keys; elapsed_.restart(); }
            }
            return true;
        }
        if (!prefix_.isEmpty()) {
            event->accept();
            if (event->type() == QEvent::KeyPress) reset();
            return true;
        }
        return false;
    }
};
}
