#pragma once
#include "export_options.hpp"
#include <QJsonArray>
#include <QJsonObject>
namespace aigle {
inline QJsonObject toJson(const ExportOptions& value) {
    QJsonObject result{{"path", value.path}};
    if (value.format) result["format"] = *value.format;
    if (value.longEdge) result["long_edge"] = *value.longEdge ? QJsonValue(**value.longEdge) : QJsonValue(QJsonValue::Null);
    if (value.output) result["output"] = *value.output;
    if (value.metadata) result["metadata"] = *value.metadata;
    if (value.sharpening) result["sharpening"] = *value.sharpening;
    if (value.artist) result["artist"] = *value.artist;
    if (value.copyright) result["copyright"] = *value.copyright;
    if (value.watermark) {
        const auto& mark = *value.watermark;
        result["watermark"] = QJsonObject{{"content", QJsonObject{{mark.kind == WatermarkOptions::Kind::Text ? "Text" : "Image", mark.content}}},
            {"anchor", mark.anchor}, {"size", mark.size}, {"margin", mark.margin}, {"opacity", mark.opacity},
            {"color", QJsonArray{mark.color[0], mark.color[1], mark.color[2]}}};
    }
    return result;
}
inline ExportOptions exportOptionsFromJson(const QJsonObject& value) {
    ExportOptions result;
    result.path = value["path"].toString();
    if (value.contains("format")) result.format = value["format"];
    if (value.contains("long_edge")) result.longEdge = value["long_edge"].isNull() ? std::optional<int>{} : std::optional<int>{value["long_edge"].toInt()};
    if (value.contains("output")) result.output = value["output"].toString();
    if (value.contains("metadata")) result.metadata = value["metadata"].toString();
    if (value.contains("sharpening")) result.sharpening = value["sharpening"].toString();
    if (value.contains("artist")) result.artist = value["artist"].toString();
    if (value.contains("copyright")) result.copyright = value["copyright"].toString();
    if (value["watermark"].isObject()) {
        const auto mark = value["watermark"].toObject(), content = mark["content"].toObject();
        const bool image = content.contains("Image");
        const auto color = mark["color"].toArray();
        result.watermark = WatermarkOptions{image ? WatermarkOptions::Kind::Image : WatermarkOptions::Kind::Text,
            content[image ? "Image" : "Text"].toString(), mark["anchor"].toString(), mark["size"].toDouble(),
            mark["margin"].toDouble(), mark["opacity"].toDouble(),
            {color.at(0).toInt(), color.at(1).toInt(), color.at(2).toInt()}};
    }
    return result;
}
}
