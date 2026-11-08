#pragma once
#include "geometry_json.hpp"
#include "editor_state.hpp"
#include "agent_json.hpp"
#include "gui_json.hpp"
#include "mask_json.hpp"
namespace aigle {
inline QStringList stringsFromJson(const QJsonArray& values) {
    QStringList result;
    for (const auto& value : values) result.append(value.toString());
    return result;
}
inline DuplicateResult duplicateResultFromJson(const QJsonObject& value) {
    DuplicateResult result;
    result.id = value["id"].toInteger();
    result.running = value["running"].toBool(true);
    result.cancelled = value["cancelled"].toBool();
    if (value.contains("error")) result.error = value["error"].toString();
    for (const auto& group : value["groups"].toArray()) result.groups.append(stringsFromJson(group.toArray()));
    for (const auto& failure : value["failures"].toArray()) {
        const auto pair = failure.toArray();
        result.failures.append({pair.at(0).toString(), pair.at(1).toString()});
    }
    return result;
}
inline EditorState editorStateFromJson(const QJsonObject& value) {
    EditorState result;
    result.open = value["open"].toBool();
    result.canUndo = value["can_undo"].toBool();
    result.canRedo = value["can_redo"].toBool();
    result.dirty = value["dirty"].toBool();
    result.retouchPreview = value["retouch_preview"].toBool();
    if (value["copy"].isDouble()) result.copy = value["copy"].toInteger();
    result.photoIdentity = value["photo_identity"].toString();
    result.document = quint64(value["document_id"].toInteger());
    result.revision = quint64(value["revision"].toInteger());
    result.display = displayDetectionFromJson(value["display_profile"].toObject());
    result.modelDiscovery = modelDiscoveryFromJson(value["model_discovery"].toObject());
    result.agent = agentStateFromJson(value["agent"].toObject());
    result.exporting = exportProgressFromJson(value["export"].toObject());
    const auto importing = value["import"].toObject();
    result.importing.running = importing["running"].toBool();
    result.importing.files = importing["files"].toInt();
    result.importing.total = importing["total"].toInt();
    if (importing.contains("error")) result.importing.error = importing["error"].toString();
    result.duplicates = duplicateResultFromJson(value["duplicates"].toObject());
    const auto extent = value["extent"].toArray();
    if (extent.size() == 2) result.extent = QSizeF(extent[0].toDouble(), extent[1].toDouble());
    const auto load = value["load"];
    if (!load.isString()) {
        const auto object = load.toObject();
        if (object.contains("Failed")) {
            result.load.status = ImageLoad::Status::Failed;
            result.load.error = object["Failed"].toString();
        } else {
            result.load.status = ImageLoad::Status::Ready;
            const auto image = object["Ready"].toObject();
            result.load.info = imageInfoFromJson(image);
            const auto size = image["source_size"].toObject();
            result.load.sourceSize = {size["w"].toInt(), size["h"].toInt()};
            const auto camera = image["camera"].toObject();
            result.load.camera.make = camera["make"].toString();
            result.load.camera.model = camera["model"].toString();
            result.load.camera.lens = camera["lens_model"].toString();
            if (camera["iso"].isDouble()) result.load.camera.iso = camera["iso"].toDouble();
            if (camera["f_number"].isDouble()) result.load.camera.aperture = camera["f_number"].toDouble();
            if (camera["focal_length"].isDouble()) result.load.camera.focalLength = camera["focal_length"].toDouble();
        }
    }
    if (value.contains("params")) {
        result.params = value["params"].toObject();
        result.retouch = retouchParametersFromJson(*result.params);
        result.geometry = geometryFromJson(*result.params);
    }
    result.retouchGuides = retouchGuidesFromJson(value["retouch_guides"].toArray());
    result.maskGuides = maskGuidesFromJson(value["mask_guides"].toArray());
    for (const auto& entry : value["history"].toArray()) {
        const auto object = entry.toObject();
        result.history.append({object["label"].toString(), object["applied"].toBool()});
    }
    result.position = value["position"].toInt();
    if (value.contains("histogram")) result.histogram = histogramFromJson(value["histogram"].toObject());
    if (value.contains("rating")) result.rating = value["rating"].toInt();
    if (value.contains("flag")) result.flag = value["flag"].toString();
    if (value.contains("label")) result.label = value["label"].toString();
    if (value.contains("tags")) result.tags = stringsFromJson(value["tags"].toArray());
    if (value.contains("iptc")) {
        result.iptc.emplace();
        const auto fields = value["iptc"].toObject();
        for (auto it = fields.begin(); it != fields.end(); ++it)
            result.iptc->insert(it.key(), it.value().toString());
    }
    result.snapshots = stringsFromJson(value["snapshots"].toArray());
    return result;
}
}
