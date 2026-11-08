#pragma once
#include <QJsonValue>
#include <QString>
namespace aigle {
struct EditOperation {
    enum class Kind { Set, Reset, ReplaceAll };
    Kind kind = Kind::Set;
    QString path;
    // Values are governed by the runtime adjustment schema.
    QJsonValue value;
};
}
