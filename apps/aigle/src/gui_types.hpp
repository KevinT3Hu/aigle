#pragma once
#include "photo_types.hpp"
#include "export_types.hpp"
#include <QMetaType>
#include <QImage>
#include <QList>
#include <QMap>
#include <QStringList>
#include <array>
#include <QRectF>
#include <QSize>
#include <QString>
#include <optional>

namespace aigle {
struct Histogram {
    bool available = false;
    QList<double> red, green, blue, luma;
};
struct ScopeData {
    bool available = false, hdr = false;
    std::array<QList<double>, 4> bins, wave;
    QList<double> vector;
    double count = 0, low = 0, high = 0, gamut = 0;
};
struct ScopeResult {
    QString requestId;
    std::optional<QString> error;
    bool available = false;
    ScopeData data;
    QList<QPointF> targets;
    double diffuseWhite = 0;
    QImage thumbnail;
};
struct ImageInfo {
    QString kind;
    std::optional<double> temperature, tint;
    bool operator==(const ImageInfo&) const = default;
};
using CurvePoints = QList<QPointF>;
// GUI values are independent of the JSON wire format used by the Rust bridge.
struct DisplayProfile {
    enum class Kind { Srgb, HdrRec2020, Icc };
    Kind kind = Kind::Srgb;
    QString icc;
    bool operator==(const DisplayProfile&) const = default;
};
struct DisplayDetection {
    bool running = false;
    QString screen, name, error;
    std::optional<DisplayProfile> profile;
    bool operator==(const DisplayDetection&) const = default;
};
struct ViewportRequest {
    QRectF region;
    QSize outputSize;
    DisplayProfile display;
    bool continuous = false;
    bool operator==(const ViewportRequest&) const = default;
};
struct RetouchDefaults {
    enum class Tool { Heal, Clone, RedEye };
    Tool tool = Tool::Heal;
    double radius = 0.015;
    double feather = 50;
    double opacity = 1;
    double pupil = 50;
    double darken = 50;
    bool operator==(const RetouchDefaults&) const = default;
};
}
Q_DECLARE_METATYPE(aigle::DisplayProfile)
Q_DECLARE_METATYPE(aigle::DisplayDetection)
Q_DECLARE_METATYPE(aigle::ViewportRequest)
Q_DECLARE_METATYPE(aigle::RetouchDefaults)

Q_DECLARE_METATYPE(aigle::CurvePoints)
