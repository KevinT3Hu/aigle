#pragma once
#include "geometry_types.hpp"
#include <QJsonObject>
namespace aigle {
inline Orientation orientationFromJson(const QJsonObject& value) {
    return {value["quarter_turns"].toInt(), value["flip"].toBool()};
}
inline CropAspect cropAspectFromJson(const QJsonObject& value) {
    return {value["kind"] == "ratio" ? CropAspect::Kind::Ratio : value["kind"] == "original" ? CropAspect::Kind::Original : CropAspect::Kind::Free,
        value["w"].toDouble(1), value["h"].toDouble(1)};
}
inline QJsonObject toJson(const Orientation& value) {
    return {{"quarter_turns", value.quarterTurns}, {"flip", value.flip}};
}
inline QJsonObject toJson(const CropAspect& value) {
    if (value.kind == CropAspect::Kind::Ratio) return {{"kind", "ratio"}, {"w", value.width}, {"h", value.height}};
    return {{"kind", value.kind == CropAspect::Kind::Original ? "original" : "free"}};
}
inline GeometryParameters geometryFromJson(const QJsonObject& params) {
    const auto geometry = params["geometry"].toObject();
    const auto crop = geometry["crop"].toObject();
    return {{crop["x"].toDouble(), crop["y"].toDouble(), crop["w"].toDouble(1), crop["h"].toDouble(1)},
        orientationFromJson(geometry["orientation_delta"].toObject()), cropAspectFromJson(geometry["aspect"].toObject()), geometry["angle"].toDouble()};
}
inline QJsonObject toJson(const GeometryParameters& value) {
    return {{"crop", QJsonObject{{"x", value.crop.x()}, {"y", value.crop.y()}, {"w", value.crop.width()}, {"h", value.crop.height()}}},
        {"orientation_delta", toJson(value.orientation)}, {"aspect", toJson(value.aspect)}, {"angle", value.angle}};
}
}
