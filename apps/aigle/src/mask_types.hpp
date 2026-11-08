#pragma once
#include <QByteArray>
#include <QList>
#include <QMetaType>
#include <QPointF>
#include <QString>
#include <array>
#include <optional>
namespace aigle {
struct MaskGuide {
    bool valid = false;
    qint64 id = 0;
    QString kind;
    // From, to, and radial Y-axis grip, in source-image coordinates.
    std::array<std::optional<QPointF>, 3> handles;
    QList<QPointF> outline;
    QByteArray coverage;
};
}
Q_DECLARE_METATYPE(aigle::MaskGuide)
