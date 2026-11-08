#pragma once
#include "mask_types.hpp"
#include "retouch_json.hpp"
namespace aigle {
inline MaskGuide maskGuideFromJson(const QJsonObject& value) {
    MaskGuide result;
    result.valid = value.contains("kind");
    result.id = value["id"].toInteger();
    result.kind = value["kind"].toString();
    const std::array<const char*, 3> keys{"from", "to", "axis_y"};
    for (size_t i = 0; i < keys.size(); ++i)
        if (value.contains(keys[i])) result.handles[i] = pointFromJson(value[keys[i]]);
    for (const auto& point : value["outline"].toArray()) result.outline.append(pointFromJson(point));
    for (const auto& sample : value["coverage"].toArray()) result.coverage.append(char(sample.toInt()));
    return result;
}
inline QList<MaskGuide> maskGuidesFromJson(const QJsonArray& values) {
    QList<MaskGuide> result;
    for (const auto& value : values) result.append(maskGuideFromJson(value.toObject()));
    return result;
}
}
