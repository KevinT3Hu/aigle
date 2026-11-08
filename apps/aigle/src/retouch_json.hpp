#pragma once
#include "retouch_types.hpp"
#include <QJsonArray>
#include <QJsonObject>
namespace aigle {
inline QPointF pointFromJson(const QJsonValue& value) {
    const auto object = value.toObject();
    return {object["x"].toDouble(), object["y"].toDouble()};
}
inline QJsonObject toJson(QPointF point) { return {{"x", point.x()}, {"y", point.y()}}; }
inline RetouchCorrection retouchCorrectionFromJson(const QJsonObject& value) {
    RetouchCorrection result;
    if (value.contains("mode")) result.mode = value["mode"].toString();
    if (value.contains("target")) result.target = pointFromJson(value["target"]);
    if (value.contains("source")) result.source = pointFromJson(value["source"]);
    if (value.contains("center")) result.center = pointFromJson(value["center"]);
    if (value.contains("radius")) result.radius = value["radius"].toDouble();
    if (value.contains("feather")) result.feather = value["feather"].toDouble();
    if (value.contains("opacity")) result.opacity = value["opacity"].toDouble();
    if (value.contains("pupil")) result.pupil = value["pupil"].toDouble();
    if (value.contains("darken")) result.darken = value["darken"].toDouble();
    return result;
}
inline QJsonObject toJson(const RetouchCorrection& value) {
    QJsonObject result;
    if (value.mode) result["mode"] = *value.mode;
    if (value.target) result["target"] = toJson(*value.target);
    if (value.source) result["source"] = toJson(*value.source);
    if (value.center) result["center"] = toJson(*value.center);
    if (value.radius) result["radius"] = *value.radius;
    if (value.feather) result["feather"] = *value.feather;
    if (value.opacity) result["opacity"] = *value.opacity;
    if (value.pupil) result["pupil"] = *value.pupil;
    if (value.darken) result["darken"] = *value.darken;
    return result;
}
inline RetouchParameters retouchParametersFromJson(const QJsonObject& value) {
    RetouchParameters result;
    for (const auto& spot : value["spots"].toArray()) result.spots.append(retouchCorrectionFromJson(spot.toObject()));
    for (const auto& eye : value["red_eye"].toArray()) result.redEye.append(retouchCorrectionFromJson(eye.toObject()));
    return result;
}
inline RetouchGuide retouchGuideFromJson(const QJsonObject& value) {
    RetouchGuide result;
    result.valid = !value.isEmpty();
    result.kind = value["kind"].toString();
    result.index = value["index"].toInt();
    result.placement = value["placement"].toBool();
    result.value = retouchCorrectionFromJson(value["value"].toObject());
    result.target = pointFromJson(value["target"]);
    result.radiusHandle = pointFromJson(value["radius_handle"]);
    if (value.contains("source")) result.source = pointFromJson(value["source"]);
    for (const auto& point : value["outline"].toArray()) result.outline.append(pointFromJson(point));
    for (const auto& point : value["source_outline"].toArray()) result.sourceOutline.append(pointFromJson(point));
    return result;
}
inline QList<RetouchGuide> retouchGuidesFromJson(const QJsonArray& values) {
    QList<RetouchGuide> result;
    for (const auto& value : values) result.append(retouchGuideFromJson(value.toObject()));
    return result;
}
inline RetouchResult retouchResultFromJson(const QJsonObject& value) {
    RetouchResult result;
    result.session = value["session"].toString();
    if (value.contains("error")) result.error = value["error"].toString();
    if (value.contains("guide")) result.guide = retouchGuideFromJson(value["guide"].toObject());
    result.placement = value["placement"].toBool();
    return result;
}
}
