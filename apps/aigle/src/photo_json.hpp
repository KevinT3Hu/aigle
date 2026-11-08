#pragma once
#include "photo_types.hpp"
#include <QJsonArray>
#include <QJsonObject>
namespace aigle {
inline PhotoIdentity photoIdentityFromJson(const QJsonObject& value) {
    return {value["path"].toString(), value["copy"].toInteger(), value["name"].toString()};
}
inline QJsonObject toJson(const PhotoIdentity& value) {
    QJsonObject result{{"path", value.path}, {"copy", value.copy ? QJsonValue(value.copy) : QJsonValue(QJsonValue::Null)}};
    if (!value.name.isNull()) result["name"] = value.name;
    return result;
}
inline QList<PhotoIdentity> photoIdentitiesFromJson(const QJsonArray& values) {
    QList<PhotoIdentity> result;
    for (const auto& value : values) result.append(photoIdentityFromJson(value.toObject()));
    return result;
}
inline QJsonArray toJson(const QList<PhotoIdentity>& values) {
    QJsonArray result;
    for (const auto& value : values) result.append(toJson(value));
    return result;
}
inline SelectionPhotoMetadata selectionMetadataFromJson(const QJsonObject& value) {
    SelectionPhotoMetadata result;
    result.loaded = value["meta_loaded"].toBool();
    for (const auto& tag : value["tags"].toArray()) result.tags.append(tag.toString());
    const auto iptc = value["iptc"].toObject();
    for (auto it = iptc.begin(); it != iptc.end(); ++it) result.iptc[it.key()] = it.value().toString();
    return result;
}
inline QList<SelectionPhotoMetadata> selectionMetadataFromJson(const QJsonArray& values) {
    QList<SelectionPhotoMetadata> result;
    for (const auto& value : values) result.append(selectionMetadataFromJson(value.toObject()));
    return result;
}

}
