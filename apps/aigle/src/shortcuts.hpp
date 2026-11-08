#pragma once
#include <QKeySequence>
namespace aigle {
inline bool shortcutContextsOverlap(const QString& a, const QString& b) {
    if (a == b) return true;
    if ((a == "dialog" && b == "comparison") || (b == "dialog" && a == "comparison")) return true;
    return (a == "canvas" && (b == "crop" || b == "retouch")) ||
           (b == "canvas" && (a == "crop" || a == "retouch"));
}
inline bool shortcutConflict(const QKeySequence& a, const QKeySequence& b) {
    return !a.isEmpty() && !b.isEmpty() &&
           (a.matches(b) != QKeySequence::NoMatch || b.matches(a) != QKeySequence::NoMatch);
}
} // namespace aigle
