#pragma once
#include "photo_json.hpp"
#include "export_options_json.hpp"
#include "geometry_json.hpp"
#include "retouch_json.hpp"
#include "commands.hpp"
#include <type_traits>

namespace aigle {
inline QStringList commandStringsFromJson(const QJsonArray& values) {
    QStringList result;
    for (const auto& value : values) result.append(value.toString());
    return result;
}
inline QMap<QString, QString> metadataFieldsFromJson(const QJsonObject& value) {
    QMap<QString, QString> result;
    for (auto it = value.begin(); it != value.end(); ++it) result[it.key()] = it.value().toString();
    return result;
}
inline QJsonObject toJson(const QMap<QString, QString>& fields) {
    QJsonObject result;
    for (auto it = fields.begin(); it != fields.end(); ++it) result[it.key()] = it.value();
    return result;
}
inline QJsonArray toJson(const QList<EditOperation>& operations) {
    QJsonArray result;
    for (const auto& operation : operations) {
        switch (operation.kind) {
        case EditOperation::Kind::Set:
            result.append(QJsonObject{{"op", "set"}, {"path", operation.path}, {"value", operation.value}}); break;
        case EditOperation::Kind::Reset:
            result.append(QJsonObject{{"op", "reset"}, {"path", operation.path}}); break;
        case EditOperation::Kind::ReplaceAll:
            result.append(QJsonObject{{"op", "replace_all"}, {"params", operation.value}}); break;
        }
    }
    return result;
}
inline QList<EditOperation> editOperationsFromJson(const QJsonArray& values) {
    QList<EditOperation> result;
    for (const auto& value : values) {
        const auto object = value.toObject();
        const auto op = object["op"].toString();
        if (op == "set") result.append({EditOperation::Kind::Set, object["path"].toString(), object["value"]});
        else if (op == "reset") result.append({EditOperation::Kind::Reset, object["path"].toString(), {}});
        else if (op == "replace_all") result.append({EditOperation::Kind::ReplaceAll, {}, object["params"]});
    }
    return result;
}
inline QJsonObject toJson(const OverlayPatch& value) {
    QJsonObject result;
    if (value.clipping) result["clipping"] = *value.clipping;
    if (value.focusPeaking) result["focus_peaking"] = *value.focusPeaking;
    if (value.gamutWarning) result["gamut_warning"] = *value.gamutWarning;
    if (value.mask) result["mask_overlay"] = *value.mask ? QJsonValue(**value.mask) : QJsonValue(QJsonValue::Null);
    if (value.proof) result["proof"] = *value.proof ? QJsonValue(**value.proof) : QJsonValue(QJsonValue::Null);
    return result;
}
inline OverlayPatch overlayPatchFromJson(const QJsonObject& value) {
    OverlayPatch result;
    if (value.contains("clipping")) result.clipping = value["clipping"].toBool();
    if (value.contains("focus_peaking")) result.focusPeaking = value["focus_peaking"].toBool();
    if (value.contains("gamut_warning")) result.gamutWarning = value["gamut_warning"].toBool();
    if (value.contains("mask_overlay")) result.mask = value["mask_overlay"].isNull() ? std::optional<qint64>{} : std::optional<qint64>{value["mask_overlay"].toInteger()};
    if (value.contains("proof")) result.proof = value["proof"].isNull() ? std::optional<QString>{} : std::optional<QString>{value["proof"].toString()};
    return result;
}
inline QJsonArray commandPointsJson(const QList<QPointF>& points) {
    QJsonArray result;
    for (const auto& point : points) result.append(toJson(point));
    return result;
}
inline QList<QPointF> commandPointsFromJson(const QJsonArray& points) {
    QList<QPointF> result;
    for (const auto& point : points) result.append(pointFromJson(point));
    return result;
}
inline QJsonObject toJson(const commands::AgentAccept& value) {
    QJsonObject result{{"cmd", "agent_accept"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::AgentCancel& value) {
    QJsonObject result{{"cmd", "agent_cancel"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::AgentCredential& value) {
    QJsonObject result{{"cmd", "agent_credential"}};
    if (value.request_id) result["request_id"] = *value.request_id;
    if (value.provider_id) result["provider_id"] = *value.provider_id;
    if (value.endpoint) result["endpoint"] = *value.endpoint;
    if (value.action) result["action"] = *value.action;
    if (value.api_key) result["api_key"] = *value.api_key;
    return result;
}
inline QJsonObject toJson(const commands::AgentModels& value) {
    QJsonObject result{{"cmd", "agent_models"}};
    if (value.request_id) result["request_id"] = *value.request_id;
    if (value.provider_id) result["provider_id"] = *value.provider_id;
    if (value.endpoint) result["endpoint"] = *value.endpoint;
    if (value.api_key) result["api_key"] = *value.api_key;
    return result;
}
inline QJsonObject toJson(const commands::AgentModelsCancel& value) {
    QJsonObject result{{"cmd", "agent_models_cancel"}};
    if (value.request_id) result["request_id"] = *value.request_id;
    return result;
}
inline QJsonObject toJson(const commands::AgentReject& value) {
    QJsonObject result{{"cmd", "agent_reject"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::AgentStart& value) {
    QJsonObject result{{"cmd", "agent_start"}};
    if (value.provider_id) result["provider_id"] = *value.provider_id;
    if (value.endpoint) result["endpoint"] = *value.endpoint;
    if (value.model) result["model"] = *value.model;
    if (value.api_key) result["api_key"] = *value.api_key;
    if (value.prompt) result["prompt"] = *value.prompt;
    return result;
}
inline QJsonObject toJson(const commands::AlbumPaths& value) {
    QJsonObject result{{"cmd", "album_paths"}};
    if (value.id) result["id"] = *value.id;
    return result;
}
inline QJsonObject toJson(const commands::Albums& value) {
    QJsonObject result{{"cmd", "albums"}};
    if (value.action) result["action"] = *value.action;
    if (value.paths) result["paths"] = QJsonArray::fromStringList(*value.paths);
    return result;
}
inline QJsonObject toJson(const commands::AutoTone& value) {
    QJsonObject result{{"cmd", "auto_tone"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::AutoWb& value) {
    QJsonObject result{{"cmd", "auto_wb"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::BatchMetadata& value) {
    QJsonObject result{{"cmd", "batch_metadata"}};
    if (value.targets) result["targets"] = toJson(*value.targets);
    if (value.stack) result["stack"] = *value.stack;
    if (value.flag) result["flag"] = *value.flag;
    if (value.rating) result["rating"] = *value.rating;
    if (value.label) result["label"] = *value.label;
    if (value.tags) result["tags"] = QJsonArray::fromStringList(*value.tags);
    if (value.remove_tags) result["remove_tags"] = QJsonArray::fromStringList(*value.remove_tags);
    if (value.tag_mode) result["tag_mode"] = *value.tag_mode;
    if (value.iptc) result["iptc"] = toJson(*value.iptc);
    return result;
}
inline QJsonObject toJson(const commands::CancelDuplicates& value) {
    QJsonObject result{{"cmd", "cancel_duplicates"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::CancelExport& value) {
    QJsonObject result{{"cmd", "cancel_export"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::CancelImport& value) {
    QJsonObject result{{"cmd", "cancel_import"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::CatalogBackup& value) {
    QJsonObject result{{"cmd", "catalog_backup"}};
    if (value.path) result["path"] = *value.path;
    return result;
}
inline QJsonObject toJson(const commands::CatalogRestore& value) {
    QJsonObject result{{"cmd", "catalog_restore"}};
    if (value.path) result["path"] = *value.path;
    return result;
}
inline QJsonObject toJson(const commands::Copies& value) {
    QJsonObject result{{"cmd", "copies"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::CreateAlbum& value) {
    QJsonObject result{{"cmd", "create_album"}};
    if (value.name) result["name"] = *value.name;
    if (value.paths) result["paths"] = QJsonArray::fromStringList(*value.paths);
    return result;
}
inline QJsonObject toJson(const commands::CreateCopies& value) {
    QJsonObject result{{"cmd", "create_copies"}};
    if (value.name) result["name"] = *value.name;
    if (value.targets) result["targets"] = toJson(*value.targets);
    return result;
}
inline QJsonObject toJson(const commands::CreateCopy& value) {
    QJsonObject result{{"cmd", "create_copy"}};
    if (value.name) result["name"] = *value.name;
    return result;
}
inline QJsonObject toJson(const commands::CropOrientation& value) {
    QJsonObject result{{"cmd", "crop_orientation"}};
    if (value.orientation) result["orientation"] = toJson(*value.orientation);
    return result;
}
inline QJsonObject toJson(const commands::DeleteAlbum& value) {
    QJsonObject result{{"cmd", "delete_album"}};
    if (value.id) result["id"] = *value.id;
    return result;
}
inline QJsonObject toJson(const commands::DetectDisplay& value) {
    QJsonObject result{{"cmd", "detect_display"}};
    if (value.screen) result["screen"] = *value.screen;
    return result;
}
inline QJsonObject toJson(const commands::Edit& value) {
    QJsonObject result{{"cmd", "edit"}};
    if (value.label) result["label"] = *value.label;
    if (value.ops) result["ops"] = toJson(*value.ops);
    if (value.gesture) result["gesture"] = *value.gesture;
    if (value.continuous) result["continuous"] = *value.continuous;
    return result;
}
inline QJsonObject toJson(const commands::Export& value) {
    QJsonObject result{{"cmd", "export"}};
    if (value.options) result["options"] = toJson(*value.options);
    if (value.overwrite) result["overwrite"] = *value.overwrite;
    if (value.source_ref) result["source_ref"] = *value.source_ref;
    if (value.params) result["params"] = *value.params;
    if (value.source_name) result["source_name"] = *value.source_name;
    if (value.job_id) result["job_id"] = *value.job_id;
    if (value.paired_sdr) result["paired_sdr"] = *value.paired_sdr;
    return result;
}
inline QJsonObject toJson(const commands::ExportInputs& value) {
    QJsonObject result{{"cmd", "export_inputs"}};
    if (value.preview) result["preview"] = *value.preview;
    if (value.current) result["current"] = *value.current;
    if (value.targets) result["targets"] = toJson(*value.targets);
    return result;
}
inline QJsonObject toJson(const commands::FileAction& value) {
    QJsonObject result{{"cmd", "file_action"}};
    if (value.action) result["action"] = *value.action;
    if (value.path) result["path"] = *value.path;
    if (value.copy) result["copy"] = *value.copy;
    if (value.destination) result["destination"] = *value.destination;
    return result;
}
inline QJsonObject toJson(const commands::FindDuplicates& value) {
    QJsonObject result{{"cmd", "find_duplicates"}};
    if (value.paths) result["paths"] = QJsonArray::fromStringList(*value.paths);
    if (value.id) result["id"] = *value.id;
    return result;
}
inline QJsonObject toJson(const commands::Goto& value) {
    QJsonObject result{{"cmd", "goto"}};
    if (value.position) result["position"] = *value.position;
    return result;
}
inline QJsonObject toJson(const commands::Import& value) {
    QJsonObject result{{"cmd", "import"}};
    if (value.source) result["source"] = *value.source;
    if (value.destination) result["destination"] = *value.destination;
    if (value.folders) result["folders"] = *value.folders;
    if (value.names) result["names"] = *value.names;
    if (value.backup) result["backup"] = *value.backup;
    return result;
}
inline QJsonObject toJson(const commands::ImportProfile& value) {
    QJsonObject result{{"cmd", "import_profile"}};
    if (value.kind) result["kind"] = *value.kind;
    if (value.path) result["path"] = *value.path;
    return result;
}
inline QJsonObject toJson(const commands::LibraryCopies& value) {
    QJsonObject result{{"cmd", "library_copies"}};
    if (value.paths) result["paths"] = QJsonArray::fromStringList(*value.paths);
    return result;
}
inline QJsonObject toJson(const commands::LibraryMetadata& value) {
    QJsonObject result{{"cmd", "library_metadata"}};
    if (value.path) result["path"] = *value.path;
    if (value.copy) result["copy"] = *value.copy;
    return result;
}
inline QJsonObject toJson(const commands::MatchLens& value) {
    QJsonObject result{{"cmd", "match_lens"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::Metadata& value) {
    QJsonObject result{{"cmd", "metadata"}};
    if (value.rating) result["rating"] = *value.rating;
    if (value.tags) result["tags"] = QJsonArray::fromStringList(*value.tags);
    if (value.iptc) result["iptc"] = toJson(*value.iptc);
    if (value.flag) result["flag"] = *value.flag;
    if (value.label) result["label"] = *value.label;
    if (value.stack) result["stack"] = *value.stack;
    if (value.remove_tags) result["remove_tags"] = QJsonArray::fromStringList(*value.remove_tags);
    if (value.tag_mode) result["tag_mode"] = *value.tag_mode;
    return result;
}
inline QJsonObject toJson(const commands::Open& value) {
    QJsonObject result{{"cmd", "open"}};
    if (value.path) result["path"] = *value.path;
    if (value.develop) result["develop"] = *value.develop;
    if (value.copy) result["copy"] = *value.copy;
    return result;
}
inline QJsonObject toJson(const commands::Overlays& value) {
    QJsonObject result{{"cmd", "overlays"}};
    if (value.overlays) result["overlays"] = toJson(*value.overlays);
    return result;
}
inline QJsonObject toJson(const commands::PasteSelected& value) {
    QJsonObject result{{"cmd", "paste_selected"}};
    if (value.params) result["params"] = *value.params;
    if (value.groups) result["groups"] = QJsonArray::fromStringList(*value.groups);
    return result;
}
inline QJsonObject toJson(const commands::Redo& value) {
    QJsonObject result{{"cmd", "redo"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::RemoteOpen& value) {
    QJsonObject result{{"cmd", "remote_open"}};
    if (value.server) result["server"] = *value.server;
    if (value.id) result["id"] = *value.id;
    return result;
}
inline QJsonObject toJson(const commands::RemoveCopy& value) {
    QJsonObject result{{"cmd", "remove_copy"}};
    if (value.id) result["id"] = *value.id;
    return result;
}
inline QJsonObject toJson(const commands::RenameAlbum& value) {
    QJsonObject result{{"cmd", "rename_album"}};
    if (value.id) result["id"] = *value.id;
    if (value.name) result["name"] = *value.name;
    return result;
}
inline QJsonObject toJson(const commands::RenameCopy& value) {
    QJsonObject result{{"cmd", "rename_copy"}};
    if (value.id) result["id"] = *value.id;
    if (value.name) result["name"] = *value.name;
    return result;
}
inline QJsonObject toJson(const commands::RestoreSnapshot& value) {
    QJsonObject result{{"cmd", "restore_snapshot"}};
    if (value.index) result["index"] = *value.index;
    return result;
}
inline QJsonObject toJson(const commands::RetouchCancel& value) {
    QJsonObject result{{"cmd", "retouch_cancel"}};
    if (value.session) result["session"] = *value.session;
    return result;
}
inline QJsonObject toJson(const commands::Save& value) {
    QJsonObject result{{"cmd", "save"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::Scan& value) {
    QJsonObject result{{"cmd", "scan"}};
    if (value.path) result["path"] = *value.path;
    if (value.recursive) result["recursive"] = *value.recursive;
    if (value.preserve_mode) result["preserve_mode"] = *value.preserve_mode;
    return result;
}
inline QJsonObject toJson(const commands::Schema& value) {
    QJsonObject result{{"cmd", "schema"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::Scopes& value) {
    QJsonObject result{{"cmd", "scopes"}};
    if (value.request_id) result["request_id"] = *value.request_id;
    if (value.target) result["target"] = *value.target;
    result["roi"] = value.roi ? QJsonValue(QJsonArray{value.roi->left(), value.roi->top(), value.roi->right(), value.roi->bottom()}) : QJsonValue(QJsonValue::Null);
    return result;
}
inline QJsonObject toJson(const commands::Snapshot& value) {
    QJsonObject result{{"cmd", "snapshot"}};
    if (value.name) result["name"] = *value.name;
    return result;
}
inline QJsonObject toJson(const commands::SyncSelected& value) {
    QJsonObject result{{"cmd", "sync_selected"}};
    if (value.params) result["params"] = *value.params;
    if (value.groups) result["groups"] = QJsonArray::fromStringList(*value.groups);
    if (value.targets) result["targets"] = toJson(*value.targets);
    return result;
}
inline QJsonObject toJson(const commands::Synthetic& value) {
    QJsonObject result{{"cmd", "synthetic"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::Tool& value) {
    QJsonObject result{{"cmd", "tool"}};
    if (value.tool) result["tool"] = *value.tool;
    if (value.angle) result["angle"] = *value.angle;
    if (value.aspect) result["aspect"] = toJson(*value.aspect);
    if (value.orientation) result["orientation"] = toJson(*value.orientation);
    if (value.normalized) result["normalized"] = *value.normalized;
    if (value.points) result["points"] = commandPointsJson(*value.points);
    if (value.id) result["id"] = *value.id;
    if (value.handle) result["handle"] = *value.handle;
    if (value.radius) result["radius"] = *value.radius;
    if (value.mask_id) result["mask_id"] = *value.mask_id;
    if (value.feather) result["feather"] = *value.feather;
    if (value.opacity) result["opacity"] = *value.opacity;
    if (value.pupil) result["pupil"] = *value.pupil;
    if (value.darken) result["darken"] = *value.darken;
    if (value.document_id) result["document_id"] = *value.document_id;
    if (value.revision) result["revision"] = *value.revision;
    return result;
}
inline QJsonObject toJson(const commands::ToolMode& value) {
    QJsonObject result{{"cmd", "tool_mode"}};
    if (value.tool) result["tool"] = *value.tool;
    return result;
}
inline QJsonObject toJson(const commands::Undo& value) {
    QJsonObject result{{"cmd", "undo"}};
    Q_UNUSED(value);
    return result;
}
inline QJsonObject toJson(const commands::UpdateAlbum& value) {
    QJsonObject result{{"cmd", "update_album"}};
    if (value.id) result["id"] = *value.id;
    if (value.paths) result["paths"] = QJsonArray::fromStringList(*value.paths);
    if (value.add) result["add"] = *value.add;
    return result;
}
inline QJsonObject toJson(const commands::Retouch& value) {
    QJsonObject result{{"cmd", value.phase == commands::Retouch::Phase::Commit ? "retouch_commit" : value.phase == commands::Retouch::Phase::Preview ? "retouch_preview" : "retouch_placement"}};
    if (value.action) result["action"] = *value.action;
    if (value.document_id) result["document_id"] = *value.document_id;
    if (value.revision) result["revision"] = *value.revision;
    if (value.session) result["session"] = *value.session;
    if (value.kind) result["kind"] = *value.kind;
    if (value.index) result["index"] = *value.index;
    if (value.expected) result["expected"] = toJson(*value.expected);
    if (value.label) result["label"] = *value.label;
    if (value.field) result["field"] = *value.field;
    if (value.value) result["value"] = *value.value;
    if (value.handle) result["handle"] = *value.handle;
    if (value.points) result["points"] = commandPointsJson(*value.points);
    if (value.all) result["all"] = *value.all;
    if (value.tool) result["tool"] = *value.tool;
    if (value.radius) result["radius"] = *value.radius;
    if (value.feather) result["feather"] = *value.feather;
    if (value.opacity) result["opacity"] = *value.opacity;
    if (value.pupil) result["pupil"] = *value.pupil;
    if (value.darken) result["darken"] = *value.darken;
    return result;
}
inline QJsonObject toJson(const EngineCommand& value) {
    return std::visit([](const auto& command) { return toJson(command); }, value);
}
inline std::optional<EngineCommand> commandFromJson(const QJsonObject& object) {
    const auto name = object["cmd"].toString();
    if (name == "agent_accept") {
        commands::AgentAccept result;
        return result;
    }
    if (name == "agent_cancel") {
        commands::AgentCancel result;
        return result;
    }
    if (name == "agent_credential") {
        commands::AgentCredential result;
        if (object.contains("request_id")) result.request_id = object["request_id"].toString();
        if (object.contains("provider_id")) result.provider_id = object["provider_id"].toString();
        if (object.contains("endpoint")) result.endpoint = object["endpoint"].toString();
        if (object.contains("action")) result.action = object["action"].toString();
        if (object.contains("api_key")) result.api_key = object["api_key"].toString();
        return result;
    }
    if (name == "agent_models") {
        commands::AgentModels result;
        if (object.contains("request_id")) result.request_id = object["request_id"].toString();
        if (object.contains("provider_id")) result.provider_id = object["provider_id"].toString();
        if (object.contains("endpoint")) result.endpoint = object["endpoint"].toString();
        if (object.contains("api_key")) result.api_key = object["api_key"].toString();
        return result;
    }
    if (name == "agent_models_cancel") {
        commands::AgentModelsCancel result;
        if (object.contains("request_id")) result.request_id = object["request_id"].toString();
        return result;
    }
    if (name == "agent_reject") {
        commands::AgentReject result;
        return result;
    }
    if (name == "agent_start") {
        commands::AgentStart result;
        if (object.contains("provider_id")) result.provider_id = object["provider_id"].toString();
        if (object.contains("endpoint")) result.endpoint = object["endpoint"].toString();
        if (object.contains("model")) result.model = object["model"].toString();
        if (object.contains("api_key")) result.api_key = object["api_key"].toString();
        if (object.contains("prompt")) result.prompt = object["prompt"].toString();
        return result;
    }
    if (name == "album_paths") {
        commands::AlbumPaths result;
        if (object.contains("id")) result.id = object["id"];
        return result;
    }
    if (name == "albums") {
        commands::Albums result;
        if (object.contains("action")) result.action = object["action"].toString();
        if (object.contains("paths")) result.paths = commandStringsFromJson(object["paths"].toArray());
        return result;
    }
    if (name == "auto_tone") {
        commands::AutoTone result;
        return result;
    }
    if (name == "auto_wb") {
        commands::AutoWb result;
        return result;
    }
    if (name == "batch_metadata") {
        commands::BatchMetadata result;
        if (object.contains("targets")) result.targets = photoIdentitiesFromJson(object["targets"].toArray());
        if (object.contains("stack")) result.stack = object["stack"];
        if (object.contains("flag")) result.flag = object["flag"].toString();
        if (object.contains("rating")) result.rating = object["rating"].toInteger();
        if (object.contains("label")) result.label = object["label"];
        if (object.contains("tags")) result.tags = commandStringsFromJson(object["tags"].toArray());
        if (object.contains("remove_tags")) result.remove_tags = commandStringsFromJson(object["remove_tags"].toArray());
        if (object.contains("tag_mode")) result.tag_mode = object["tag_mode"].toString();
        if (object.contains("iptc")) result.iptc = metadataFieldsFromJson(object["iptc"].toObject());
        return result;
    }
    if (name == "cancel_duplicates") {
        commands::CancelDuplicates result;
        return result;
    }
    if (name == "cancel_export") {
        commands::CancelExport result;
        return result;
    }
    if (name == "cancel_import") {
        commands::CancelImport result;
        return result;
    }
    if (name == "catalog_backup") {
        commands::CatalogBackup result;
        if (object.contains("path")) result.path = object["path"].toString();
        return result;
    }
    if (name == "catalog_restore") {
        commands::CatalogRestore result;
        if (object.contains("path")) result.path = object["path"].toString();
        return result;
    }
    if (name == "copies") {
        commands::Copies result;
        return result;
    }
    if (name == "create_album") {
        commands::CreateAlbum result;
        if (object.contains("name")) result.name = object["name"].toString();
        if (object.contains("paths")) result.paths = commandStringsFromJson(object["paths"].toArray());
        return result;
    }
    if (name == "create_copies") {
        commands::CreateCopies result;
        if (object.contains("name")) result.name = object["name"].toString();
        if (object.contains("targets")) result.targets = photoIdentitiesFromJson(object["targets"].toArray());
        return result;
    }
    if (name == "create_copy") {
        commands::CreateCopy result;
        if (object.contains("name")) result.name = object["name"].toString();
        return result;
    }
    if (name == "crop_orientation") {
        commands::CropOrientation result;
        if (object.contains("orientation")) result.orientation = orientationFromJson(object["orientation"].toObject());
        return result;
    }
    if (name == "delete_album") {
        commands::DeleteAlbum result;
        if (object.contains("id")) result.id = object["id"];
        return result;
    }
    if (name == "detect_display") {
        commands::DetectDisplay result;
        if (object.contains("screen")) result.screen = object["screen"].toString();
        return result;
    }
    if (name == "edit") {
        commands::Edit result;
        if (object.contains("label")) result.label = object["label"].toString();
        if (object.contains("ops")) result.ops = editOperationsFromJson(object["ops"].toArray());
        if (object.contains("gesture")) result.gesture = object["gesture"].toInteger();
        if (object.contains("continuous")) result.continuous = object["continuous"].toBool();
        return result;
    }
    if (name == "export") {
        commands::Export result;
        if (object.contains("options")) result.options = exportOptionsFromJson(object["options"].toObject());
        if (object.contains("overwrite")) result.overwrite = object["overwrite"].toBool();
        if (object.contains("source_ref")) result.source_ref = object["source_ref"];
        if (object.contains("params")) result.params = object["params"].toObject();
        if (object.contains("source_name")) result.source_name = object["source_name"].toString();
        if (object.contains("job_id")) result.job_id = object["job_id"].toString();
        if (object.contains("paired_sdr")) result.paired_sdr = object["paired_sdr"].toBool();
        return result;
    }
    if (name == "export_inputs") {
        commands::ExportInputs result;
        if (object.contains("preview")) result.preview = object["preview"].toBool();
        if (object.contains("current")) result.current = object["current"].toBool();
        if (object.contains("targets")) result.targets = photoIdentitiesFromJson(object["targets"].toArray());
        return result;
    }
    if (name == "file_action") {
        commands::FileAction result;
        if (object.contains("action")) result.action = object["action"].toString();
        if (object.contains("path")) result.path = object["path"].toString();
        if (object.contains("copy")) result.copy = object["copy"];
        if (object.contains("destination")) result.destination = object["destination"].toString();
        return result;
    }
    if (name == "find_duplicates") {
        commands::FindDuplicates result;
        if (object.contains("paths")) result.paths = commandStringsFromJson(object["paths"].toArray());
        if (object.contains("id")) result.id = object["id"];
        return result;
    }
    if (name == "goto") {
        commands::Goto result;
        if (object.contains("position")) result.position = object["position"].toInteger();
        return result;
    }
    if (name == "import") {
        commands::Import result;
        if (object.contains("source")) result.source = object["source"].toString();
        if (object.contains("destination")) result.destination = object["destination"].toString();
        if (object.contains("folders")) result.folders = object["folders"].toString();
        if (object.contains("names")) result.names = object["names"].toString();
        if (object.contains("backup")) result.backup = object["backup"].toString();
        return result;
    }
    if (name == "import_profile") {
        commands::ImportProfile result;
        if (object.contains("kind")) result.kind = object["kind"].toString();
        if (object.contains("path")) result.path = object["path"].toString();
        return result;
    }
    if (name == "library_copies") {
        commands::LibraryCopies result;
        if (object.contains("paths")) result.paths = commandStringsFromJson(object["paths"].toArray());
        return result;
    }
    if (name == "library_metadata") {
        commands::LibraryMetadata result;
        if (object.contains("path")) result.path = object["path"].toString();
        if (object.contains("copy")) result.copy = object["copy"];
        return result;
    }
    if (name == "match_lens") {
        commands::MatchLens result;
        return result;
    }
    if (name == "metadata") {
        commands::Metadata result;
        if (object.contains("rating")) result.rating = object["rating"].toInteger();
        if (object.contains("tags")) result.tags = commandStringsFromJson(object["tags"].toArray());
        if (object.contains("iptc")) result.iptc = metadataFieldsFromJson(object["iptc"].toObject());
        if (object.contains("flag")) result.flag = object["flag"].toString();
        if (object.contains("label")) result.label = object["label"];
        if (object.contains("stack")) result.stack = object["stack"];
        if (object.contains("remove_tags")) result.remove_tags = commandStringsFromJson(object["remove_tags"].toArray());
        if (object.contains("tag_mode")) result.tag_mode = object["tag_mode"].toString();
        return result;
    }
    if (name == "open") {
        commands::Open result;
        if (object.contains("path")) result.path = object["path"].toString();
        if (object.contains("develop")) result.develop = object["develop"].toBool();
        if (object.contains("copy")) result.copy = object["copy"];
        return result;
    }
    if (name == "overlays") {
        commands::Overlays result;
        if (object.contains("overlays")) result.overlays = overlayPatchFromJson(object["overlays"].toObject());
        return result;
    }
    if (name == "paste_selected") {
        commands::PasteSelected result;
        if (object.contains("params")) result.params = object["params"].toObject();
        if (object.contains("groups")) result.groups = commandStringsFromJson(object["groups"].toArray());
        return result;
    }
    if (name == "redo") {
        commands::Redo result;
        return result;
    }
    if (name == "remote_open") {
        commands::RemoteOpen result;
        if (object.contains("server")) result.server = object["server"].toString();
        if (object.contains("id")) result.id = object["id"];
        return result;
    }
    if (name == "remove_copy") {
        commands::RemoveCopy result;
        if (object.contains("id")) result.id = object["id"];
        return result;
    }
    if (name == "rename_album") {
        commands::RenameAlbum result;
        if (object.contains("id")) result.id = object["id"];
        if (object.contains("name")) result.name = object["name"].toString();
        return result;
    }
    if (name == "rename_copy") {
        commands::RenameCopy result;
        if (object.contains("id")) result.id = object["id"];
        if (object.contains("name")) result.name = object["name"].toString();
        return result;
    }
    if (name == "restore_snapshot") {
        commands::RestoreSnapshot result;
        if (object.contains("index")) result.index = object["index"].toInteger();
        return result;
    }
    if (name == "retouch_cancel") {
        commands::RetouchCancel result;
        if (object.contains("session")) result.session = object["session"].toString();
        return result;
    }
    if (name == "retouch_commit") {
        commands::Retouch result;
        result.phase = commands::Retouch::Phase::Commit;
        if (object.contains("action")) result.action = object["action"].toString();
        if (object.contains("document_id")) result.document_id = object["document_id"].toInteger();
        if (object.contains("revision")) result.revision = object["revision"].toInteger();
        if (object.contains("session")) result.session = object["session"].toString();
        if (object.contains("kind")) result.kind = object["kind"].toString();
        if (object.contains("index")) result.index = object["index"].toInteger();
        if (object.contains("expected")) result.expected = retouchCorrectionFromJson(object["expected"].toObject());
        if (object.contains("label")) result.label = object["label"].toString();
        if (object.contains("field")) result.field = object["field"].toString();
        if (object.contains("value")) result.value = object["value"];
        if (object.contains("handle")) result.handle = object["handle"].toString();
        if (object.contains("points")) result.points = commandPointsFromJson(object["points"].toArray());
        if (object.contains("all")) result.all = object["all"].toBool();
        return result;
    }
    if (name == "retouch_placement") {
        commands::Retouch result;
        result.phase = commands::Retouch::Phase::Placement;
        if (object.contains("action")) result.action = object["action"].toString();
        if (object.contains("document_id")) result.document_id = object["document_id"].toInteger();
        if (object.contains("revision")) result.revision = object["revision"].toInteger();
        if (object.contains("session")) result.session = object["session"].toString();
        if (object.contains("kind")) result.kind = object["kind"].toString();
        if (object.contains("index")) result.index = object["index"].toInteger();
        if (object.contains("expected")) result.expected = retouchCorrectionFromJson(object["expected"].toObject());
        if (object.contains("handle")) result.handle = object["handle"].toString();
        if (object.contains("points")) result.points = commandPointsFromJson(object["points"].toArray());
        if (object.contains("tool")) result.tool = object["tool"].toString();
        if (object.contains("radius")) result.radius = object["radius"].toDouble();
        if (object.contains("feather")) result.feather = object["feather"].toDouble();
        if (object.contains("opacity")) result.opacity = object["opacity"].toDouble();
        if (object.contains("pupil")) result.pupil = object["pupil"].toDouble();
        if (object.contains("darken")) result.darken = object["darken"].toDouble();
        return result;
    }
    if (name == "retouch_preview") {
        commands::Retouch result;
        result.phase = commands::Retouch::Phase::Preview;
        if (object.contains("action")) result.action = object["action"].toString();
        if (object.contains("document_id")) result.document_id = object["document_id"].toInteger();
        if (object.contains("revision")) result.revision = object["revision"].toInteger();
        if (object.contains("session")) result.session = object["session"].toString();
        if (object.contains("kind")) result.kind = object["kind"].toString();
        if (object.contains("index")) result.index = object["index"].toInteger();
        if (object.contains("expected")) result.expected = retouchCorrectionFromJson(object["expected"].toObject());
        if (object.contains("field")) result.field = object["field"].toString();
        if (object.contains("value")) result.value = object["value"];
        if (object.contains("label")) result.label = object["label"].toString();
        if (object.contains("handle")) result.handle = object["handle"].toString();
        if (object.contains("points")) result.points = commandPointsFromJson(object["points"].toArray());
        return result;
    }
    if (name == "save") {
        commands::Save result;
        return result;
    }
    if (name == "scan") {
        commands::Scan result;
        if (object.contains("path")) result.path = object["path"].toString();
        if (object.contains("recursive")) result.recursive = object["recursive"].toBool();
        if (object.contains("preserve_mode")) result.preserve_mode = object["preserve_mode"].toBool();
        return result;
    }
    if (name == "schema") {
        commands::Schema result;
        return result;
    }
    if (name == "scopes") {
        commands::Scopes result;
        if (object.contains("request_id")) result.request_id = object["request_id"].toString();
        if (object.contains("target")) result.target = object["target"].toInteger();
        if (object["roi"].isArray()) { const auto r = object["roi"].toArray(); result.roi = QRectF(QPointF(r.at(0).toDouble(), r.at(1).toDouble()), QPointF(r.at(2).toDouble(), r.at(3).toDouble())); }
        return result;
    }
    if (name == "snapshot") {
        commands::Snapshot result;
        if (object.contains("name")) result.name = object["name"].toString();
        return result;
    }
    if (name == "sync_selected") {
        commands::SyncSelected result;
        if (object.contains("params")) result.params = object["params"].toObject();
        if (object.contains("groups")) result.groups = commandStringsFromJson(object["groups"].toArray());
        if (object.contains("targets")) result.targets = photoIdentitiesFromJson(object["targets"].toArray());
        return result;
    }
    if (name == "synthetic") {
        commands::Synthetic result;
        return result;
    }
    if (name == "tool") {
        commands::Tool result;
        if (object.contains("tool")) result.tool = object["tool"].toString();
        if (object.contains("angle")) result.angle = object["angle"].toDouble();
        if (object.contains("aspect")) result.aspect = cropAspectFromJson(object["aspect"].toObject());
        if (object.contains("orientation")) result.orientation = orientationFromJson(object["orientation"].toObject());
        if (object.contains("normalized")) result.normalized = object["normalized"].toBool();
        if (object.contains("points")) result.points = commandPointsFromJson(object["points"].toArray());
        if (object.contains("id")) result.id = object["id"];
        if (object.contains("handle")) result.handle = object["handle"].toString();
        if (object.contains("radius")) result.radius = object["radius"].toDouble();
        if (object.contains("mask_id")) result.mask_id = object["mask_id"].toInteger();
        if (object.contains("feather")) result.feather = object["feather"].toDouble();
        if (object.contains("opacity")) result.opacity = object["opacity"].toDouble();
        if (object.contains("pupil")) result.pupil = object["pupil"].toDouble();
        if (object.contains("darken")) result.darken = object["darken"].toDouble();
        if (object.contains("document_id")) result.document_id = object["document_id"].toInteger();
        if (object.contains("revision")) result.revision = object["revision"].toInteger();
        return result;
    }
    if (name == "tool_mode") {
        commands::ToolMode result;
        if (object.contains("tool")) result.tool = object["tool"].toString();
        return result;
    }
    if (name == "undo") {
        commands::Undo result;
        return result;
    }
    if (name == "update_album") {
        commands::UpdateAlbum result;
        if (object.contains("id")) result.id = object["id"];
        if (object.contains("paths")) result.paths = commandStringsFromJson(object["paths"].toArray());
        if (object.contains("add")) result.add = object["add"].toBool();
        return result;
    }
    return std::nullopt;
}
}
