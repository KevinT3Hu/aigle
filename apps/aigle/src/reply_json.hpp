#pragma once
#include "replies.hpp"
#include "editor_state_json.hpp"
#include "export_json.hpp"
#include "library_json.hpp"
namespace aigle {
inline OpenResult openResultFromJson(const QJsonObject& value) {
    return {value["path"].toString(), value["copy"].toInteger(), value["id"].toInteger(),
        value["develop"].toBool(true), stringsFromJson(value["warnings"].toArray())};
}
inline PhotoMetadata photoMetadataFromJson(const QJsonObject& value) {
    PhotoMetadata result{photoIdentityFromJson(value), libraryPhotoFromJson(value), value["stack"].toString(), {}};
    if (!value["metadata_error"].isNull() && !value["metadata_error"].isUndefined()) result.error = value["metadata_error"].toString();
    return result;
}
inline CopyRecord copyRecordFromJson(const QJsonObject& value) {
    return {value["id"].toInteger(), value["path"].toString(), value["name"].toString()};
}
inline QList<CopyRecord> copyRecordsFromJson(const QJsonArray& values) {
    QList<CopyRecord> result;
    for (const auto& value : values) result.append(copyRecordFromJson(value.toObject()));
    return result;
}
inline QList<QPair<QString, QString>> failuresFromJson(const QJsonArray& values) {
    QList<QPair<QString, QString>> result;
    for (const auto& value : values) {
        const auto object = value.toObject();
        result.append({object["path"].toString(), object["error"].toString()});
    }
    return result;
}
inline AlbumRecord albumFromJson(const QJsonObject& value) {
    return {value["id"].toInteger(), value["name"].toString(), stringsFromJson(value["paths"].toArray())};
}
inline EngineReply replyFromJson(const QString& command, const QJsonValue& value) {
    EngineReply result{command, {}};
    const auto object = value.toObject();
    if (command.startsWith("retouch_")) result.data = retouchResultFromJson(object);
    else if (command == "scopes") result.data = scopeResultFromJson(object);
    else if (command == "agent_models") result.data = modelDiscoveryFromJson(object);
    else if (command == "agent_credential") result.data = credentialResultFromJson(object);
    else if (command == "tool") result.data = ToolResult{object.contains("created_mask") ? std::optional<qint64>{object["created_mask"].toInteger()} : std::nullopt};
    else if (command == "import_profile" || command == "match_lens") result.data = ProfileResult{object["name"].toString(), object["camera_model"].toString()};
    else if (command == "catalog_restore") result.data = CatalogRestoreResult{object["recovery"].toString(), object["error"].toString(), object["restored"].toBool(), object["rollback_error"].isString() ? std::optional<QString>{object["rollback_error"].toString()} : std::nullopt};
    else if (command == "sync_selected" || command == "batch_metadata") {
        BatchResult batch;
        for (const auto& item : object["updated"].toArray())
            batch.updated.append(photoMetadataFromJson(item.isObject() ? item.toObject() : QJsonObject{{"path", item}}));
        batch.failures = failuresFromJson(object["failures"].toArray());
        result.data = batch;
    } else if (command == "export_inputs") result.data = ExportInputsResult{exportInputsFromJson(value.toArray()), object.contains("error") ? std::optional<QString>{object["error"].toString()} : std::nullopt};
    else if (command == "export") result.data = exportProgressFromJson(object);
    else if (command == "find_duplicates") {
        auto duplicate = duplicateResultFromJson(object);
        if (duplicate.error) duplicate.running = false;
        result.data = duplicate;
    } else if (command == "file_action") {
        FileActionResult file;
        file.photo = photoIdentityFromJson(object);
        file.action = object["action"].toString();
        file.ok = object["ok"].toBool(); file.closed = object["closed"].toBool();
        if (object.contains("error")) file.error = object["error"].toString();
        if (object.contains("reopen_error")) file.reopenError = object["reopen_error"].toString();
        if (object["opened"].isObject()) file.opened = openResultFromJson(object["opened"].toObject());
        result.data = file;
    } else if (command == "library_copies" || command == "copies") result.data = copyRecordsFromJson(value.toArray());
    else if (command == "library_metadata") result.data = photoMetadataFromJson(object);
    else if (command == "schema") result.data = SchemaDocument{object};
    else if (command == "scan" || command == "scan_develop") result.data = stringsFromJson(value.toArray());
    else if (command == "album_paths" || command == "update_album" || command == "rename_album" || command == "delete_album") result.data = albumFromJson(object);
    else if (command == "open" || command == "synthetic" || command == "remote_open" || command == "remove_copy") result.data = openResultFromJson(object);
    else if (command == "create_copy" || command == "rename_copy") result.data = copyRecordFromJson(object);
    else if (command == "create_copies") result.data = CreateCopiesResult{copyRecordsFromJson(object["copies"].toArray()), failuresFromJson(object["errors"].toArray())};
    else if (command == "create_album") result.data = value.toInteger();
    else if (command == "albums") {
        AlbumChoices choices;
        choices.action = object["action"].toString("open");
        choices.paths = stringsFromJson(object["paths"].toArray());
        for (const auto& item : object["albums"].toArray()) {
            const auto pair = item.toArray();
            choices.albums.append({pair.at(0).toInteger(), pair.at(1).toString(), {}});
        }
        result.data = choices;
    }
    return result;
}
// Metadata is still persisted in item roles using the library's wire schema.
inline QJsonObject toJson(const PhotoMetadata& value) {
    auto result = toJson(value.identity);
    const auto& facts = value.facts;
    result["meta_loaded"] = facts.metadataLoaded;
    result["facts_loaded"] = facts.factsLoaded;
    if (facts.rating) result["rating"] = *facts.rating;
    result["flag"] = facts.flag;
    result["label"] = facts.label.isNull() ? QJsonValue(QJsonValue::Null) : QJsonValue(facts.label);
    result["camera"] = facts.camera; result["lens"] = facts.lens;
    if (facts.captured) result["captured"] = *facts.captured;
    if (facts.modified) result["modified"] = *facts.modified;
    result["tags"] = QJsonArray::fromStringList(facts.tags);
    result["keywords"] = QJsonArray::fromStringList(facts.keywords);
    QJsonObject iptc;
    for (auto it = facts.iptc.begin(); it != facts.iptc.end(); ++it) iptc[it.key()] = it.value();
    result["iptc"] = iptc;
    result["stack"] = value.stack;
    if (value.error) result["metadata_error"] = *value.error;
    return result;
}
}
