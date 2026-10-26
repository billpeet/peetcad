using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;
using SolidWorks.Interop.sldworks;
using SolidWorks.Interop.swconst;
using ComTypes = System.Runtime.InteropServices.ComTypes;

namespace PeetSwExport;

using Json = Dictionary<string, object?>;

/// <summary>
/// Writes what a SolidWorks part or assembly is made of (sketches, feature definitions, dimension values,
/// bodies, mates) as JSON, for PeetCAD's importer to read. Files are opened read-only in a private
/// SolidWorks instance and are never saved. Lengths are metres and angles radians, as the API gives them.
/// </summary>
internal static class Program
{
    /// <summary>Raise this when the output changes shape, so the importer can tell exports apart.</summary>
    private const int FormatVersion = 5;
    private const string FlatPatternSuffix = "SM-FLAT-PATTERN";
    private const int MaxRestarts = 5;

    private static string outputDirectory = string.Empty;
    private static bool exportStep;
    private static bool exportDxf;
    private static bool readSelections = true;
    private static ModelDoc2? currentDoc;

    /// <summary>Every feature data interface of the API. Which of them a feature's definition answers to is asked of the object.</summary>
    private static readonly Type[] DataInterfaces = LoadableTypes(typeof(SldWorks).Assembly)
        .Where(t => t.IsInterface && t.IsImport && Regex.IsMatch(t.Name, "^I[A-Z].*FeatureData"))
        .OrderBy(t => t.Name, StringComparer.Ordinal)
        .ToArray();
    private static readonly Dictionary<string, Type[]> InterfacesByFeatureType = new();

    private static readonly JsonSerializerOptions JsonOptions = new() {
        NumberHandling = JsonNumberHandling.AllowNamedFloatingPointLiterals,
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull
    };

    [STAThread]
    public static int Main(string[] args)
    {
        var positional = args.Where(a => !a.StartsWith("--")).ToArray();
        var flags = args.Where(a => a.StartsWith("--")).ToArray();
        if (positional.Length != 2 || flags.Except(new[] { "--step", "--dxf", "--no-selections" }).Any()) {
            Console.Error.WriteLine("Usage: peet-sw-export.exe <input-list.txt> <output-directory> [--step] [--dxf] [--no-selections]");
            return 2;
        }
        exportStep = flags.Contains("--step");
        exportDxf = flags.Contains("--dxf");
        readSelections = !flags.Contains("--no-selections");

        outputDirectory = Path.GetFullPath(positional[1]);
        Directory.CreateDirectory(outputDirectory);

        SldWorks? sw = null;
        var ownsApplication = false;
        var results = new List<Json>();
        var total = Stopwatch.StartNew();
        string? fatal = null;
        try {
            var inputs = File.ReadAllLines(positional[0]).Select(l => l.Trim()).Where(l => l.Length > 0).ToArray();

            sw = Start();
            ownsApplication = true;

            var restarts = 0;
            foreach (var input in inputs) {
                results.Add(Export(sw, Path.GetFullPath(input)));
                if (IsAlive(sw))
                    continue;
                // A document took SolidWorks down with it. What was read is written; the rest get a new instance.
                if (++restarts > MaxRestarts)
                    throw new InvalidOperationException($"SolidWorks stopped more than {MaxRestarts} times.");
                Log("SolidWorks has stopped. Starting it again.");
                ownsApplication = false;
                var stopped = sw;
                sw = null;
                Try(() => Marshal.FinalReleaseComObject(stopped));
                GC.Collect();
                GC.WaitForPendingFinalizers();
                System.Threading.Thread.Sleep(5000);
                sw = Start();
                ownsApplication = true;
            }
        }
        catch (Exception ex) {
            Log("FATAL " + ex);
            fatal = ex.Message;
        }
        finally {
            currentDoc = null;
            if (sw != null) {
                // Feature and dimension wrappers keep SolidWorks alive after ExitApp unless they are collected first.
                GC.Collect();
                GC.WaitForPendingFinalizers();
                if (ownsApplication) {
                    Log("Closing the private SolidWorks instance");
                    var running = sw;
                    Try(() => { running.ExitApp(); return null; });
                }
                var instance = sw;
                Try(() => Marshal.FinalReleaseComObject(instance));
            }
        }

        File.WriteAllText(Path.Combine(outputDirectory, "summary.json"), JsonSerializer.Serialize(
            new Json { ["seconds"] = total.Elapsed.TotalSeconds, ["fatalError"] = fatal, ["results"] = results }, JsonOptions));
        return fatal == null && results.All(r => (bool)r["completed"]!) ? 0 : 1;
    }

    private static SldWorks Start()
    {
        Log("Activating a private SolidWorks instance");
        var sw = (SldWorks)Activator.CreateInstance(Type.GetTypeFromProgID("SldWorks.Application", true)!)!;
        if (sw.UserControl || ArrayOf(sw.GetDocuments()).Length > 0)
            throw new InvalidOperationException("SolidWorks is already under user control or has documents open. Use a dedicated worker session.");
        sw.Visible = false;
        Log("Connected to SolidWorks " + sw.RevisionNumber());
        return sw;
    }

    private static bool IsAlive(SldWorks sw)
    {
        try {
            sw.RevisionNumber();
            return true;
        }
        catch (Exception) {
            return false;
        }
    }

    /// <summary>Always writes a file: when the export fails part way, it says so and holds what was read.</summary>
    private static Json Export(SldWorks sw, string path)
    {
        var timer = Stopwatch.StartNew();
        var isAssembly = Path.GetExtension(path).Equals(".sldasm", StringComparison.OrdinalIgnoreCase);
        var configurations = new List<Json>();
        var root = new Json {
            ["formatVersion"] = FormatVersion,
            ["solidWorksRevision"] = sw.RevisionNumber(),
            ["name"] = Path.GetFileNameWithoutExtension(path),
            ["documentType"] = isAssembly ? "assembly" : "part",
            ["sourcePath"] = path,
            ["units"] = "metres and radians",
            ["configurations"] = configurations
        };

        var fileName = Path.GetFileName(path) + ".swx.json";
        ModelDoc2? doc = null;
        string? error = null;
        try {
            Log("Opening " + path);
            var hash = Sha256(path);
            root["sourceSha256"] = hash;

            int errors = 0, warnings = 0;
            var type = isAssembly ? swDocumentTypes_e.swDocASSEMBLY : swDocumentTypes_e.swDocPART;
            doc = sw.OpenDoc6(path, (int)type, (int)(swOpenDocOptions_e.swOpenDocOptions_Silent | swOpenDocOptions_e.swOpenDocOptions_ReadOnly), "", ref errors, ref warnings);
            if (doc == null)
                throw new InvalidOperationException($"OpenDoc6 failed, error={errors}, warning={warnings}");
            if (!doc.IsOpenedReadOnly())
                throw new InvalidOperationException("SolidWorks did not open the document read-only.");
            currentDoc = doc;

            root["displayLengthUnit"] = doc.LengthUnit;
            root["activeConfiguration"] = doc.ConfigurationManager.ActiveConfiguration.Name;

            foreach (var name in ArrayOf(doc.GetConfigurationNames()).Cast<string>()) {
                Log("Configuration " + name);
                if (doc.ConfigurationManager.ActiveConfiguration.Name != name
                    && (!doc.ShowConfiguration2(name) || doc.ConfigurationManager.ActiveConfiguration.Name != name))
                    throw new InvalidOperationException("Cannot activate configuration " + name);
                configurations.Add(DumpConfiguration(doc, name, isAssembly, Path.GetFileName(path)));
            }

            if (Sha256(path) != hash)
                error = "The file changed while it was being exported.";

            // Last, and after the file is written once: exporting a flat pattern is what has taken SolidWorks down.
            // A failure here leaves the flat pattern out. It does not make the document's export incomplete.
            if (exportDxf && !isAssembly && error == null) try {
                Write();
                foreach (var configuration in configurations.Where(c => c["sheetMetal"] is true)) {
                    var name = (string)configuration["name"]!;
                    // The export leaves the document closed behind it, so it is opened again for the next one
                    if (Try(() => doc.GetTitle()) == null) {
                        Try(() => Marshal.FinalReleaseComObject(doc));
                        int openErrors = 0, openWarnings = 0;
                        doc = sw.OpenDoc6(path, (int)swDocumentTypes_e.swDocPART,
                            (int)(swOpenDocOptions_e.swOpenDocOptions_Silent | swOpenDocOptions_e.swOpenDocOptions_ReadOnly), "", ref openErrors, ref openWarnings);
                        if (doc == null || !doc.IsOpenedReadOnly())
                            throw new InvalidOperationException($"The document could not be opened again, error={openErrors}");
                    }
                    // ShowConfiguration2 is false for the configuration that is already shown
                    if (doc.ConfigurationManager.ActiveConfiguration.Name != name && !doc.ShowConfiguration2(name))
                        continue;
                    var dxf = Path.GetFileName(path) + "." + SafeFileName(name) + ".flat.dxf";
                    var target = Path.Combine(outputDirectory, dxf);
                    // Where the flat pattern goes in the drawing: an origin, then the x, y and z directions.
                    var alignment = new double[] { 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1 };
                    // 1: the flat pattern's outline and holes. 4: bend lines.
                    // On a document opened read-only SolidWorks writes the whole file and then faults, so what
                    // counts is whether the file is there.
                    string? fault = null;
                    try {
                        if (!((PartDoc)doc).ExportToDWG2(target, path, (int)swExportToDWG_e.swExportToDWG_ExportSheetMetal,
                                true, alignment, false, false, 1 | 4, null))
                            fault = "SolidWorks reported that it failed";
                    }
                    catch (COMException ex) {
                        fault = ex.Message;
                    }
                    var written = File.Exists(target) && new FileInfo(target).Length > 0;
                    configuration["flatDxf"] = written ? dxf : null;
                    configuration["flatDxfFault"] = fault;
                    Log($"  Flat pattern of {name}: {(written ? dxf : "not written")}{(fault == null ? "" : " (" + fault + ")")}");
                    if (!IsAlive(sw))
                        throw new InvalidOperationException("SolidWorks stopped while exporting the flat pattern of " + name);
                }
            }
            catch (Exception ex) {
                Log("  Flat patterns stopped: " + ex.Message);
                root["flatDxfError"] = ex.Message;
            }
        }
        catch (Exception ex) {
            Log("ERROR " + ex);
            error = ex.Message;
        }
        finally {
            currentDoc = null;
            if (doc != null) {
                var opened = doc;
                Try(() => { sw.CloseDoc(opened.GetTitle()); return null; });
                Try(() => Marshal.FinalReleaseComObject(opened));
            }
        }

        Write();
        Log($"Wrote {fileName} in {timer.Elapsed.TotalSeconds:0.00}s, completed={error == null}");

        void Write()
        {
            root["completed"] = error == null;
            root["error"] = error;
            root["exportedUtc"] = DateTime.UtcNow.ToString("o");
            File.WriteAllText(Path.Combine(outputDirectory, fileName), JsonSerializer.Serialize(root, JsonOptions));
        }

        return new Json { ["path"] = path, ["file"] = fileName, ["completed"] = error == null, ["error"] = error, ["seconds"] = timer.Elapsed.TotalSeconds };
    }

    private static Json DumpConfiguration(ModelDoc2 doc, string name, bool isAssembly, string fileName)
    {
        var result = new Json { ["name"] = name };
        if (doc.GetConfigurationByName(name) is Configuration configuration) {
            result["description"] = configuration.Description;
            result["derived"] = configuration.IsDerived();
            result["parent"] = configuration.GetParent()?.Name;
        }

        // SolidWorks makes this configuration itself to hold the unfolded part. Its bodies give the flat size;
        // its features are those of the configuration it derives from.
        var flatPattern = name.EndsWith(FlatPatternSuffix, StringComparison.OrdinalIgnoreCase);
        result["flatPattern"] = flatPattern;
        if (flatPattern) {
            // Not every such configuration shows the part flat: its bodies are the flat ones only if it does
            var flat = ArrayOf(doc.FeatureManager.GetFeatures(false)).OfType<Feature>().FirstOrDefault(f => f.GetTypeName2() == "FlatPattern");
            var state = flat == null ? Array.Empty<object>() : ArrayOf(flat.IsSuppressed2((int)swInConfigurationOpts_e.swThisConfiguration, null));
            result["unfolded"] = state.Length > 0 && !Convert.ToBoolean(state[0]);
        }

        if (!flatPattern) {
            // The whole list is read first: reading a definition's selections rolls the model back and forward.
            var seen = new HashSet<int>();
            var tree = new List<Node>();
            for (var feature = (Feature?)doc.FirstFeature(); feature != null; feature = (Feature?)feature.GetNextFeature())
                AddNode(feature, tree, seen);
            // The feature manager's own list, for what the walk does not reach (the sheet metal folder, for one)
            foreach (var feature in ArrayOf(doc.FeatureManager.GetFeatures(false)).OfType<Feature>())
                AddNode(feature, tree, seen);
            result["features"] = tree.Select(n => DumpFeature(doc, n)).ToList();

            var equations = doc.GetEquationMgr();
            var list = new List<Json>();
            if (equations != null)
                for (var i = 0; i < equations.GetCount(); i++)
                    list.Add(new Json {
                        ["index"] = i,
                        ["expression"] = equations.Equation[i],
                        ["value"] = Try(() => (object)equations.Value[i]),
                        ["globalVariable"] = equations.GlobalVariable[i],
                        ["disabled"] = equations.Disabled[i]
                    });
            result["equations"] = list;
        }

        if (isAssembly) {
            if (!flatPattern)
                result["components"] = ArrayOf(((AssemblyDoc)doc).GetComponents(false)).Cast<Component2>().Select(c => new Json {
                    ["path"] = c.Name2,
                    ["file"] = c.GetPathName(),
                    ["configuration"] = c.ReferencedConfiguration,
                    ["suppression"] = c.GetSuppression(),
                    ["fixed"] = c.IsFixed(),
                    ["virtual"] = c.IsVirtual,
                    // 16 numbers: a 3x3 rotation by rows, the translation, the scale, and three unused
                    ["transform"] = Try(() => c.Transform2?.ArrayData)
                }).ToList();
        }
        else {
            var part = (PartDoc)doc;
            result["material"] = Try(() => (object)part.GetMaterialPropertyName2(name, out _));
            var bodies = ArrayOf(part.GetBodies2((int)swBodyType_e.swSolidBody, false)).Cast<Body2>().ToArray();
            result["bodies"] = bodies.Select(b => DumpBody(doc, b)).ToList();

            result["sheetMetal"] = !flatPattern && bodies.Any(b => b.IsSheetMetal());
            if (exportStep && !flatPattern)
                result["step"] = SaveCopy(doc, fileName + "." + SafeFileName(name) + ".step");
        }

        return result;
    }

    private sealed class Node
    {
        public Node(Feature feature) => Feature = feature;
        public Feature Feature { get; }
        public List<Node> Children { get; } = new();
        /// <summary>Every sub-feature by name, those written elsewhere in the tree too.</summary>
        public List<string> SubFeatures { get; } = new();
        public bool SubFeaturesElsewhere { get; set; }
    }

    private static void AddNode(Feature feature, List<Node> into, HashSet<int> seen)
    {
        if (!seen.Add(feature.GetID()))
            return;
        var node = new Node(feature);
        into.Add(node);
        for (var child = (Feature?)feature.GetFirstSubFeature(); child != null; child = (Feature?)child.GetNextSubFeature()) {
            node.SubFeatures.Add(child.Name);
            // The walk meets a library feature's contents before the library feature itself, as features of
            // their own: they stay where they are, and the library feature lists them by name
            if (seen.Contains(child.GetID()))
                node.SubFeaturesElsewhere = true;
            AddNode(child, node.Children, seen);
        }
    }

    private static Json DumpFeature(ModelDoc2 doc, Node node)
    {
        var feature = node.Feature;
        var type = feature.GetTypeName2();
        var suppression = ArrayOf(feature.IsSuppressed2((int)swInConfigurationOpts_e.swThisConfiguration, null));
        var suppressed = suppression.Length > 0 && Convert.ToBoolean(suppression[0]);
        var errorCode = feature.GetErrorCode2(out var isWarning);

        var result = new Json {
            ["name"] = feature.Name,
            ["type"] = type,
            ["id"] = feature.GetID(),
            ["suppressed"] = suppressed,
            ["errorCode"] = errorCode == 0 ? null : errorCode,
            ["isWarning"] = isWarning ? true : null,
            ["parents"] = NamesOf(Try(() => feature.GetParents())),
            ["children"] = NamesOf(Try(() => feature.GetChildren()))
        };

        var dimensions = new List<Json>();
        var guard = 0;
        for (var display = (DisplayDimension?)feature.GetFirstDisplayDimension(); display != null && ++guard < 10000;
             display = (DisplayDimension?)feature.GetNextDisplayDimension(display)) {
            var dimension = (Dimension?)display.GetDimension2(0);
            if (dimension == null)
                continue;
            dimensions.Add(new Json {
                ["fullName"] = dimension.FullName,
                ["name"] = dimension.Name,
                ["value"] = First(Try(() => dimension.GetSystemValue3((int)swInConfigurationOpts_e.swThisConfiguration, null))),
                // swDimensionParamType_e: 0 linear (a length), 1 angular, 2 a count
                ["valueType"] = dimension.GetType(),
                // swDimensionType_e: how it is drawn (linear, radial, diameter, angular, ...)
                ["displayType"] = display.Type2,
                // The direction its dimension line runs: along the sketch's x, its y, or from one point to the
                // other. The same two points can be dimensioned all three ways.
                ["lineDirection"] = Try(() => dimension.DimensionLineDirection?.ArrayData),
                ["extensionDirection"] = Try(() => dimension.ExtensionLineDirection?.ArrayData),
                // swDimensionDrivenState_e: 1 driven, 2 driving
                ["drivenState"] = dimension.DrivenState,
                ["readOnly"] = dimension.ReadOnly
            });
        }
        if (dimensions.Count > 0)
            result["dimensions"] = dimensions;
        else if (type == "CornerRelief") {
            // A corner relief shows no dimensions and the API has no definition of one (only the calls that
            // make one): its size may still be a dimension by name
            var named = new List<Json>();
            for (var i = 1; i <= 6; i++) {
                var name = "D" + i;
                if (Try(() => feature.Parameter(name)) is Dimension dimension)
                    named.Add(new Json {
                        ["fullName"] = dimension.FullName,
                        ["name"] = name,
                        ["value"] = First(Try(() => dimension.GetSystemValue3((int)swInConfigurationOpts_e.swThisConfiguration, null))),
                        ["valueType"] = dimension.GetType()
                    });
            }
            if (named.Count > 0)
                result["namedDimensions"] = named;
        }

        var specific = Try(() => feature.GetSpecificFeature2());
        if (specific is Sketch sketch)
            result["sketch"] = TryJson(() => DumpSketch(sketch), "sketch of " + feature.Name);
        else if (specific is RefPlane plane)
            // Model space to the plane's own space: its normal is the third row of the rotation
            result["planeTransform"] = Try(() => plane.Transform?.ArrayData);
        else if (specific is Mate2 mate)
            result["mate"] = TryJson(() => DumpMate(mate), "mate " + feature.Name);

        var definition = TryJson(() => DumpDefinition(doc, feature, type, suppressed), "definition of " + feature.Name);
        if (definition != null)
            result["definition"] = definition;

        if (node.Children.Count > 0)
            result["sub"] = node.Children.Select(c => DumpFeature(doc, c)).ToList();
        if (node.SubFeaturesElsewhere)
            result["subFeatures"] = node.SubFeatures;
        return result;
    }

    /// <summary>
    /// Reads every value a feature's definition offers. The API has one interface per kind of feature; reading them
    /// by reflection means no value is left out because nobody wrote the line that reads it.
    /// </summary>
    private static Json? DumpDefinition(ModelDoc2 doc, Feature feature, string type, bool suppressed)
    {
        var definition = feature.GetDefinition();
        if (definition == null)
            return null;

        if (!InterfacesByFeatureType.TryGetValue(type, out var interfaces)) {
            var matched = new List<Type>();
            var unknown = Marshal.GetIUnknownForObject(definition);
            try {
                foreach (var candidate in DataInterfaces) {
                    var iid = candidate.GUID;
                    if (Marshal.QueryInterface(unknown, ref iid, out var pointer) == 0) {
                        Marshal.Release(pointer);
                        matched.Add(candidate);
                    }
                }
            }
            finally {
                Marshal.Release(unknown);
            }
            InterfacesByFeatureType[type] = interfaces = matched.ToArray();
            Log($"  {type}: {(interfaces.Length == 0 ? "no data interface" : string.Join(", ", interfaces.Select(i => i.Name)))}");
        }
        // What the running SolidWorks says the definition is, which can be newer than the interop assemblies
        // this is built against: their members are read by name (late bound) after the interop's own
        var dispatch = DispatchTypeOf(definition);
        if (interfaces.Length == 0 && (dispatch == null || dispatch.Members.Count == 0))
            return null;

        var values = new Json();
        var failed = new List<string>();
        var result = new Json { ["interfaces"] = interfaces.Select(i => i.Name).ToList(), ["values"] = values };
        if (dispatch != null)
            result["dispatchType"] = dispatch.Name;

        // The edges, faces and sketches a definition refers to can only be read between these two calls, which
        // roll the model back to the feature and forward again. A suppressed feature has nothing to roll back to.
        Action? release = null;
        if (readSelections) {
            var owner = interfaces.FirstOrDefault(i => i.GetMethod("AccessSelections") != null && i.GetMethod("ReleaseSelectionAccess") != null);
            if (owner != null) {
                var accessed = Try(() => owner.GetMethod("AccessSelections")!.Invoke(definition, new object?[] { doc, null }));
                result["selectionsRead"] = accessed is true;
                if (accessed is true)
                    release = () => owner.GetMethod("ReleaseSelectionAccess")!.Invoke(definition, null);
            }
            else if (dispatch != null && dispatch.Has("AccessSelections") && dispatch.Has("ReleaseSelectionAccess")) {
                var accessed = Try(() => InvokeLate(definition, "AccessSelections", false, doc, new DispatchWrapper(null)));
                result["selectionsRead"] = accessed is true;
                if (accessed is true)
                    release = () => InvokeLate(definition, "ReleaseSelectionAccess", false);
            }
        }

        try {
            foreach (var type_ in interfaces) {
                foreach (var property in type_.GetProperties()) {
                    // IGetX and IX are the pointer forms of GetX and X, for C++ callers
                    if (!property.CanRead || Regex.IsMatch(property.Name, "^I[A-Z]"))
                        continue;
                    var index = property.GetIndexParameters();
                    if (index.Length == 0)
                        Read(property.Name, () => property.GetValue(definition, null));
                    // A value per reference (a plane's constraint on each of up to three selections)
                    else if (index.Length == 1 && index[0].ParameterType == typeof(int))
                        for (var i = 0; i < 3; i++) {
                            var at = i;
                            Read($"{property.Name}[{i}]", () => property.GetValue(definition, new object[] { at }));
                        }
                }
                foreach (var method in type_.GetMethods()) {
                    // IsBossFeature is the only thing that tells a boss-extrude from a cut-extrude
                    if (method.IsSpecialName || !(method.Name.StartsWith("Get") || Regex.IsMatch(method.Name, "^Is[A-Z]")) || method.ReturnType == typeof(void))
                        continue;
                    var parameters = method.GetParameters();
                    // A method that gives a second value through its one argument (a reference axis's
                    // GetSelections gives the selections, and their types through the argument)
                    if (parameters.Length == 1 && parameters[0].ParameterType == typeof(object).MakeByRefType()) {
                        var given = new object?[] { null };
                        Read(method.Name, () => method.Invoke(definition, given));
                        if (values.ContainsKey(method.Name))
                            values[method.Name + ".out"] = Describe(given[0]);
                        continue;
                    }
                    // A flag (forward or reverse, first or second direction) is tried both ways. What an index or
                    // an out parameter means differs from method to method, so those are left alone.
                    if (parameters.Length > 2 || parameters.Any(p => p.ParameterType != typeof(bool)))
                        continue;
                    foreach (var arguments in FlagCombinations(parameters.Length))
                        Read(method.Name + (arguments.Length == 0 ? "" : "(" + string.Join(",", arguments.Select(a => (bool)a ? "true" : "false")) + ")"),
                            () => method.Invoke(definition, arguments));
                }
            }
            // What this SolidWorks offers that the interop assemblies don't know (the "up to reference" of a
            // linear pattern, from SolidWorks 2019, for one), and the values per index (a pattern's
            // transform of each instance)
            if (dispatch != null) {
                var late = ReadLate(definition, dispatch, values, failed);
                if (late.Count > 0)
                    result["lateBound"] = late;
            }
        }
        finally {
            if (release != null)
                Try(() => { release(); return null; });
        }

        if (failed.Count > 0)
            result["unreadable"] = failed;
        return result;

        void Read(string key, Func<object?> read)
        {
            if (values.ContainsKey(key))
                return;
            try {
                values[key] = Describe(read());
            }
            catch (Exception) {
                failed.Add(key);
            }
        }
    }

    // ---- Late binding: what the running SolidWorks offers, by name ----
    //
    // The interop assemblies the exporter is built against can be older than the SolidWorks it runs on, and
    // then have no member for what was added since. Every SolidWorks object also answers IDispatch, whose type
    // information lists the members this SolidWorks has: those are read by name. To export something new,
    // nothing needs adding here if it is a value on a definition (or on an object a definition gives): it is
    // read and written under the API's own name. Indexed reads are limited to the names in IndexedReads.

    [ComImport, Guid("00020400-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IDispatchInfo
    {
        [PreserveSig] int GetTypeInfoCount(out int count);
        [PreserveSig] int GetTypeInfo(int index, int lcid, out ComTypes.ITypeInfo? info);
    }

    /// <summary>A member that reads a value: a property, or a method taking nothing, a flag or an index.</summary>
    private sealed class LateMember
    {
        public LateMember(string name, bool property, VarEnum argument) => (Name, Property, Argument) = (name, property, argument);
        public string Name { get; }
        public bool Property { get; }
        public VarEnum Argument { get; }
    }

    private sealed class DispatchType
    {
        public DispatchType(string name) => Name = name;
        public string Name { get; }
        public List<LateMember> Members { get; } = new();
        public HashSet<string> All { get; } = new(StringComparer.Ordinal);
        public bool Has(string name) => All.Contains(name);
    }

    private static readonly Dictionary<string, DispatchType> DispatchTypes = new(StringComparer.Ordinal);

    /// <summary>Methods taking an index that are read for each index from 0 until one fails or gives nothing.</summary>
    private static readonly Regex IndexedReads = new("^(GetTransform|Get[A-Za-z]*AtIndex|GetPathSegmentAt)$");
    /// <summary>Methods taking an index (an enumeration) that are read for each of its values, from 0.</summary>
    private static readonly Dictionary<string, int> FixedIndexes = new(StringComparer.Ordinal) {
        // A fillet's faces by set (swSimpleFilletWhichFaces_e): 3 to 5 are a full round fillet's
        ["GetFaces"] = 6
    };
    private const int MaxIndex = 256;
    private const int MaxDescribeDepth = 2;
    [ThreadStatic] private static int describeDepth;

    /// <summary>What the object's IDispatch says it is and which of its members read a value. Null if it can't say.</summary>
    private static DispatchType? DispatchTypeOf(object? value)
    {
        if (value == null || !Marshal.IsComObject(value))
            return null;
        try {
            if (value is not IDispatchInfo dispatch || dispatch.GetTypeInfoCount(out var count) != 0 || count < 1)
                return null;
            if (dispatch.GetTypeInfo(0, 0, out var info) != 0 || info == null)
                return null;
            info.GetDocumentation(-1, out var name, out _, out _, out _);
            if (DispatchTypes.TryGetValue(name, out var known))
                return known;
            var type = new DispatchType(name);
            info.GetTypeAttr(out var attrPointer);
            ComTypes.TYPEATTR attr;
            try {
                attr = Marshal.PtrToStructure<ComTypes.TYPEATTR>(attrPointer);
            }
            finally {
                info.ReleaseTypeAttr(attrPointer);
            }
            var elementSize = Marshal.SizeOf<ComTypes.ELEMDESC>();
            for (var i = 0; i < attr.cFuncs; i++) {
                info.GetFuncDesc(i, out var funcPointer);
                try {
                    var func = Marshal.PtrToStructure<ComTypes.FUNCDESC>(funcPointer);
                    if ((func.wFuncFlags & (short)ComTypes.FUNCFLAGS.FUNCFLAG_FRESTRICTED) != 0)
                        continue;
                    info.GetDocumentation(func.memid, out var member, out _, out _, out _);
                    type.All.Add(member);
                    // IGetX and IX are the pointer forms of GetX and X, for C++ callers
                    if (Regex.IsMatch(member, "^I[A-Z]"))
                        continue;
                    // The arguments, less a dual interface's hidden return value; any out argument rules it out
                    var arguments = new List<VarEnum>();
                    var returns = false;
                    var output = false;
                    for (var k = 0; k < func.cParams; k++) {
                        var element = Marshal.PtrToStructure<ComTypes.ELEMDESC>(func.lprgelemdescParam + k * elementSize);
                        var flags = element.desc.paramdesc.wParamFlags;
                        if ((flags & ComTypes.PARAMFLAG.PARAMFLAG_FRETVAL) != 0)
                            returns = true;
                        else if ((flags & ComTypes.PARAMFLAG.PARAMFLAG_FOUT) != 0)
                            output = true;
                        else
                            arguments.Add((VarEnum)element.tdesc.vt);
                    }
                    var result = (VarEnum)func.elemdescFunc.tdesc.vt;
                    returns = returns || (result != VarEnum.VT_VOID && result != VarEnum.VT_HRESULT);
                    if (!returns || output || arguments.Count > 1)
                        continue;
                    var argument = arguments.Count == 0 ? VarEnum.VT_EMPTY : arguments[0];
                    if (argument is not (VarEnum.VT_EMPTY or VarEnum.VT_BOOL or VarEnum.VT_I4 or VarEnum.VT_I2 or VarEnum.VT_INT))
                        continue;
                    var property = func.invkind == ComTypes.INVOKEKIND.INVOKE_PROPERTYGET;
                    if (!property && (func.invkind != ComTypes.INVOKEKIND.INVOKE_FUNC || !Regex.IsMatch(member, "^(Get|Is)[A-Z]")))
                        continue;
                    // An indexed value only where the index is known to count instances or items
                    if (argument is (VarEnum.VT_I4 or VarEnum.VT_I2 or VarEnum.VT_INT) && !IndexedReads.IsMatch(member) && !FixedIndexes.ContainsKey(member))
                        continue;
                    if (type.Members.All(m => m.Name != member))
                        type.Members.Add(new LateMember(member, property, argument));
                }
                finally {
                    info.ReleaseFuncDesc(funcPointer);
                }
            }
            DispatchTypes[name] = type;
            Log($"  {name}: {type.Members.Count} members by name");
            return type;
        }
        catch (Exception ex) {
            Log("  The type of an object could not be read: " + ex.Message);
            return null;
        }
    }

    private static object? InvokeLate(object target, string name, bool property, params object?[] arguments) =>
        target.GetType().InvokeMember(name, property ? BindingFlags.GetProperty : BindingFlags.InvokeMethod, null, target, arguments);

    /// <summary>
    /// Reads the members of <paramref name="dispatch"/> that <paramref name="values"/> doesn't have yet, under the
    /// names the interop path gives them (<c>Name</c>, <c>Name(true)</c>, <c>Name[2]</c>). Returns the names read.
    /// </summary>
    private static List<string> ReadLate(object target, DispatchType dispatch, Json values, List<string> failed)
    {
        var read = new List<string>();
        foreach (var member in dispatch.Members) {
            switch (member.Argument) {
                case VarEnum.VT_EMPTY:
                    One(member.Name, () => InvokeLate(target, member.Name, member.Property));
                    break;
                case VarEnum.VT_BOOL:
                    foreach (var flag in new[] { true, false })
                        One($"{member.Name}({(flag ? "true" : "false")})", () => InvokeLate(target, member.Name, member.Property, flag));
                    break;
                default:
                    if (values.ContainsKey(member.Name + "[0]"))
                        break;
                    // Every value of an enumeration, or each index until one fails or gives nothing
                    var all = FixedIndexes.TryGetValue(member.Name, out var count);
                    for (var i = 0; i < (all ? count : MaxIndex); i++) {
                        object? value;
                        try {
                            value = InvokeLate(target, member.Name, member.Property, i);
                        }
                        catch (Exception) {
                            if (all)
                                continue;
                            break;
                        }
                        if (value == null) {
                            if (all)
                                continue;
                            break;
                        }
                        values[$"{member.Name}[{i}]"] = Describe(value);
                        read.Add($"{member.Name}[{i}]");
                    }
                    break;
            }
        }
        return read;

        void One(string key, Func<object?> get)
        {
            if (values.ContainsKey(key))
                return;
            try {
                values[key] = Describe(get());
                read.Add(key);
            }
            catch (Exception) {
                failed.Add(key);
            }
        }
    }

    /// <summary>An object the exporter has no case for, by its type's name and the values it reads, some levels deep.</summary>
    private static Json DescribeUnknown(object value)
    {
        var dispatch = DispatchTypeOf(value);
        var result = new Json { ["kind"] = dispatch == null ? "unknown" : "object", ["type"] = dispatch?.Name };
        if (dispatch == null || describeDepth >= MaxDescribeDepth)
            return result;
        describeDepth++;
        try {
            var values = new Json();
            var failed = new List<string>();
            ReadLate(value, dispatch, values, failed);
            result["values"] = values;
            if (failed.Count > 0)
                result["unreadable"] = failed;
        }
        finally {
            describeDepth--;
        }
        return result;
    }

    private static IEnumerable<object[]> FlagCombinations(int count) => count switch {
        0 => new[] { Array.Empty<object>() },
        1 => new[] { new object[] { true }, new object[] { false } },
        _ => new[] { new object[] { true, true }, new object[] { true, false }, new object[] { false, true }, new object[] { false, false } }
    };

    /// <summary>Coordinates are in the sketch's own plane (z is 0 in a flat sketch): modelToSketch relates them to the model.</summary>
    private static Json DumpSketch(Sketch sketch)
    {
        var result = new Json {
            ["is3D"] = sketch.Is3D(),
            ["modelToSketch"] = sketch.ModelToSketchTransform?.ArrayData,
            // swConstrainedStatus_e: 2 under defined, 3 fully defined, 4 over defined
            ["constrainedStatus"] = sketch.GetConstrainedStatus()
        };
        var entityType = 0;
        var on = Try(() => sketch.GetReferenceEntity(ref entityType));
        result["on"] = Describe(on);
        result["blockInstances"] = ArrayOf(Try(() => sketch.GetSketchBlockInstances())).Length;

        result["points"] = ArrayOf(sketch.GetSketchPoints2()).Cast<SketchPoint>().Select(Point).ToList();

        result["segments"] = Segments(sketch);

        // A block is a group of segments drawn once and placed any number of times. The sketch's own list
        // of segments leaves out what is inside one, so each instance is written with its definition's
        // segments (in the definition's own coordinates) and the transform that places them in this sketch.
        var blocks = new List<Json>();
        foreach (var instance in ArrayOf(Try(() => sketch.GetSketchBlockInstances())).OfType<SketchBlockInstance>()) {
            var inner = Try(() => instance.Definition?.GetSketch()) as Sketch;
            blocks.Add(new Json {
                ["name"] = instance.Name,
                ["definition"] = (inner as Feature)?.Name,
                ["angle"] = instance.Angle,
                ["scale"] = instance.Scale2,
                ["blockToSketch"] = Try(() => instance.BlockToSketchTransform?.ArrayData),
                ["segments"] = inner == null ? null : TryList(() => Segments(inner)),
                ["points"] = inner == null ? null : TryList(() => ArrayOf(inner.GetSketchPoints2()).Cast<SketchPoint>().Select(Point).ToList())
            });
        }
        if (blocks.Count > 0)
            result["blocks"] = blocks;

        var relations = new List<Json>();
        foreach (var relation in ArrayOf(sketch.RelationManager?.GetRelations((int)swSketchRelationFilterType_e.swAll)).Cast<SketchRelation>()) {
            var display = Try(() => relation.GetDisplayDimension()) as DisplayDimension;
            relations.Add(new Json {
                // swConstraintType_e
                ["type"] = ((swConstraintType_e)relation.GetRelationType()).ToString().Replace("swConstraintType_", "").ToLowerInvariant(),
                ["entities"] = ArrayOf(Try(() => relation.GetEntities())).Select(Describe).ToList(),
                // What the relation is defined by when that is outside the sketch: the model edge a converted
                // line follows, for one
                ["defined"] = Defined(relation),
                ["suppressed"] = relation.Suppressed ? true : null,
                ["dimension"] = ((Dimension?)display?.GetDimension2(0))?.FullName
            });
        }
        result["relations"] = relations;
        return result;
    }

    private static List<Json> Segments(Sketch sketch)
    {
        var segments = new List<Json>();
        foreach (var segment in ArrayOf(sketch.GetSketchSegments()).Cast<SketchSegment>()) {
            var kind = (swSketchSegments_e)segment.GetType();
            var item = new Json {
                ["id"] = segment.GetID(),
                ["type"] = kind.ToString().Replace("swSketch", "").ToLowerInvariant(),
                ["name"] = segment.GetName(),
                ["construction"] = segment.ConstructionGeometry,
                ["length"] = segment.GetLength()
            };
            switch (kind) {
                case swSketchSegments_e.swSketchLINE:
                    var line = (SketchLine)segment;
                    item["start"] = Point(line.GetStartPoint2());
                    item["end"] = Point(line.GetEndPoint2());
                    break;
                case swSketchSegments_e.swSketchARC:
                    var arc = (SketchArc)segment;
                    item["center"] = Point(arc.GetCenterPoint2());
                    item["start"] = Point(arc.GetStartPoint2());
                    item["end"] = Point(arc.GetEndPoint2());
                    item["radius"] = arc.GetRadius();
                    item["circle"] = arc.IsCircle() == 1;
                    // 1 counter-clockwise, -1 clockwise, seen from the sketch's normal
                    item["direction"] = arc.GetRotationDir();
                    break;
                case swSketchSegments_e.swSketchELLIPSE:
                    var ellipse = (SketchEllipse)segment;
                    item["center"] = Point(ellipse.GetCenterPoint2());
                    item["major"] = Point(ellipse.GetMajorPoint2());
                    item["minor"] = Point(ellipse.GetMinorPoint2());
                    item["start"] = Point(ellipse.GetStartPoint2());
                    item["end"] = Point(ellipse.GetEndPoint2());
                    break;
                case swSketchSegments_e.swSketchSPLINE:
                    item["points"] = ArrayOf(((SketchSpline)segment).GetPoints2()).Select(Point).ToList();
                    break;
                case swSketchSegments_e.swSketchTEXT:
                    var text = (SketchText)segment;
                    item["text"] = text.Text;
                    item["format"] = TryJson(() => TextFormatOf(text), "format of a text");
                    break;
            }
            segments.Add(item);
        }
        return segments;
    }

    /// <summary>
    /// A sketch text's font and style. Heights and lengths in metres, angles in radians; the factors are 1 as
    /// the font has it.
    /// </summary>
    private static Json? TextFormatOf(SketchText text)
    {
        if (text.GetTextFormat() is not TextFormat format)
            return null;
        return new Json {
            // Whether the text follows the document's font rather than its own
            ["useDocumentFormat"] = Try(() => text.GetUseDocTextFormat()),
            ["typeFace"] = Try(() => format.TypeFaceName),
            ["bold"] = Try(() => format.Bold),
            ["italic"] = Try(() => format.Italic),
            ["underline"] = Try(() => format.Underline),
            ["strikeout"] = Try(() => format.Strikeout),
            ["charHeight"] = Try(() => format.CharHeight),
            ["charHeightInPoints"] = Try(() => format.IsHeightSpecifiedInPts() ? format.CharHeightInPts : null),
            ["widthFactor"] = Try(() => format.WidthFactor),
            ["charSpacingFactor"] = Try(() => format.CharSpacingFactor),
            ["lineSpacing"] = Try(() => format.LineSpacing),
            ["lineLength"] = Try(() => format.LineLength),
            ["obliqueAngle"] = Try(() => format.ObliqueAngle),
            ["escapement"] = Try(() => format.Escapement),
            ["upsideDown"] = Try(() => format.UpsideDown),
            ["backwards"] = Try(() => format.BackWards),
            ["vertical"] = Try(() => format.Vertical),
            ["coordinates"] = Try(() => text.GetCoordinates())
        };
    }

    private static List<object?>? Defined(SketchRelation relation)
    {
        var outside = ArrayOf(Try(() => relation.GetDefinitionEntities2())).Where(e => e is not ISketchSegment && e is not ISketchPoint)
            .Select(Describe).ToList();
        return outside.Count == 0 ? null : outside;
    }

    private static Json? Point(object? value) => value is SketchPoint point
        ? new Json { ["id"] = point.GetID(), ["x"] = point.X, ["y"] = point.Y, ["z"] = point.Z == 0 ? null : point.Z }
        : null;

    private static Json DumpMate(Mate2 mate)
    {
        var entities = new List<Json>();
        for (var i = 0; i < mate.GetMateEntityCount(); i++) {
            var entity = mate.MateEntity(i);
            if (entity == null)
                continue;
            entities.Add(new Json {
                ["component"] = entity.ReferenceComponent?.Name2,
                // swSelectType_e
                ["referenceType"] = entity.ReferenceType2,
                // A point and a direction in the assembly's space, then a radius or two, depending on the type
                ["params"] = entity.EntityParams,
                ["entity"] = Describe(Try(() => entity.Reference))
            });
        }
        return new Json {
            // swMateType_e and swMateAlign_e
            ["type"] = ((swMateType_e)mate.Type).ToString().Replace("swMate", "").ToLowerInvariant(),
            ["alignment"] = mate.Alignment,
            ["flipped"] = mate.Flipped,
            ["entities"] = entities
        };
    }

    private static Json DumpBody(ModelDoc2 doc, Body2 body)
    {
        // For a density of 1: centre of gravity, volume, area, mass, then the moments of inertia
        var mass = Try(() => body.GetMassProperties(1.0)) as double[];
        return new Json {
            ["name"] = body.Name,
            ["sheetMetal"] = body.IsSheetMetal(),
            ["box"] = body.GetBodyBox(),
            // GetBodyBox can be loose (by up to a hundredth of a millimetre round a freeform face): the body's
            // own extreme points along -x, -y, -z, +x, +y, +z give the tight one, in the box's order
            ["tightBox"] = TightBox(body),
            ["volume"] = mass is { Length: > 3 } ? mass[3] : null,
            ["area"] = mass is { Length: > 4 } ? mass[4] : null,
            ["centreOfGravity"] = mass is { Length: > 2 } ? mass.Take(3).ToArray() : null,
            ["edgeCount"] = body.GetEdgeCount(),
            ["faces"] = ArrayOf(body.GetFaces()).Cast<Face2>().Select(Face).ToList(),
            // Every face is listed, since they are what a rebuilt part is compared with. Of the edges, the named ones.
            ["namedEdges"] = ArrayOf(body.GetEdges()).Cast<Edge>().Where(e => !string.IsNullOrWhiteSpace(doc.GetEntityName(e))).Select(Edge).ToList()
        };
    }

    private static double[]? TightBox(Body2 body)
    {
        var box = new double[6];
        for (var axis = 0; axis < 3; axis++) {
            foreach (var sign in new[] { -1.0, 1.0 }) {
                var d = new double[3];
                d[axis] = sign;
                double x = 0, y = 0, z = 0;
                var found = false;
                try {
                    found = body.GetExtremePoint(d[0], d[1], d[2], out x, out y, out z);
                }
                catch (Exception) {
                }
                if (!found)
                    return null;
                box[axis + (sign < 0 ? 0 : 3)] = new[] { x, y, z }[axis];
            }
        }
        return box;
    }

    private static Json Face(Face2 face)
    {
        var result = new Json { ["kind"] = "face" };
        var name = currentDoc?.GetEntityName(face);
        result["name"] = string.IsNullOrWhiteSpace(name) ? null : name;
        result["feature"] = (Try(() => face.GetFeature()) as Feature)?.Name;
        result["area"] = face.GetArea();
        var box = face.GetBox() as double[];
        result["box"] = box;
        // A point that is on the face (the middle of its box may be in a hole or off a bent face): for picking it
        if (box is { Length: 6 })
            result["point"] = (Try(() => face.GetClosestPointOn((box[0] + box[3]) / 2, (box[1] + box[4]) / 2, (box[2] + box[5]) / 2)) as double[])?.Take(3).ToArray();
        if (face.GetSurface() is Surface surface) {
            result["surface"] = ((swSurfaceTypes_e)surface.Identity()).ToString().Replace("_TYPE", "").ToLowerInvariant();
            if (surface.IsPlane()) {
                // The plane's normal, then a point on it. The face's own normal points out of the body.
                result["plane"] = surface.PlaneParams;
                result["normal"] = face.Normal;
            }
            else if (surface.IsCylinder())
                // A point on the axis, the axis, the radius
                result["cylinder"] = surface.CylinderParams;
        }
        return result;
    }

    private static Json Edge(Edge edge)
    {
        var result = new Json { ["kind"] = "edge" };
        var name = currentDoc?.GetEntityName(edge);
        result["name"] = string.IsNullOrWhiteSpace(name) ? null : name;
        // A closed edge (a full circle) has no ends
        result["start"] = (edge.GetStartVertex() as Vertex)?.GetPoint();
        result["end"] = (edge.GetEndVertex() as Vertex)?.GetPoint();
        if (edge.GetCurve() is Curve curve) {
            if (curve.IsLine())
                result["curve"] = "line";
            else if (curve.IsCircle()) {
                result["curve"] = "circle";
                // Centre, axis, radius
                result["circle"] = curve.CircleParams;
            }
            else
                result["curve"] = "other";
        }
        result["between"] = ArrayOf(Try(() => edge.GetTwoAdjacentFaces2())).OfType<Face2>()
            .Select(f => (Try(() => f.GetFeature()) as Feature)?.Name).ToList();
        return result;
    }

    /// <summary>A value from the API as JSON: numbers and text as they are, and the things a definition refers to by what identifies them.</summary>
    private static object? Describe(object? value)
    {
        switch (value) {
            case null:
            case string _:
            case bool _:
                return value;
            case Enum e:
                return Convert.ToInt32(e);
            case double[] _:
            case int[] _:
                return value;
            case Array array:
                return array.Cast<object?>().Select(Describe).ToList();
        }
        if (value.GetType().IsPrimitive)
            return value;
        if (!Marshal.IsComObject(value))
            return value.ToString();

        try {
            switch (value) {
                case IFeature feature:
                    return new Json { ["kind"] = "feature", ["name"] = feature.Name, ["type"] = feature.GetTypeName2() };
                case IFace2 face:
                    return Face((Face2)face);
                case IEdge edge:
                    return Edge((Edge)edge);
                case IVertex vertex:
                    return new Json { ["kind"] = "vertex", ["point"] = vertex.GetPoint() };
                // With where it is, in its sketch's own coordinates: an id alone can be the id of something else
                // (the origin and what a relation refers to outside the sketch are not in the sketch's lists)
                case ISketchSegment segment: {
                    var described = new Json {
                        ["kind"] = "sketchSegment", ["id"] = segment.GetID(), ["sketch"] = (segment.GetSketch() as Feature)?.Name,
                        ["type"] = ((swSketchSegments_e)segment.GetType()).ToString().Replace("swSketch", "").ToLowerInvariant()
                    };
                    if (segment is ISketchLine line) {
                        described["start"] = Point(line.GetStartPoint2());
                        described["end"] = Point(line.GetEndPoint2());
                    }
                    else if (segment is ISketchArc arc) {
                        described["center"] = Point(arc.GetCenterPoint2());
                        described["start"] = Point(arc.GetStartPoint2());
                        described["end"] = Point(arc.GetEndPoint2());
                        described["radius"] = arc.GetRadius();
                    }
                    return described;
                }
                case ISketchPoint point:
                    return new Json {
                        ["kind"] = "sketchPoint", ["id"] = point.GetID(), ["sketch"] = (point.GetSketch() as Feature)?.Name,
                        ["x"] = point.X, ["y"] = point.Y, ["z"] = point.Z == 0 ? null : point.Z
                    };
                case ISketch sketch:
                    return new Json { ["kind"] = "sketch", ["name"] = (sketch as Feature)?.Name };
                case IRefPlane plane:
                    return new Json { ["kind"] = "plane", ["name"] = (plane as Feature)?.Name, ["transform"] = plane.Transform?.ArrayData };
                case IComponent2 component:
                    return new Json { ["kind"] = "component", ["path"] = component.Name2 };
                case IBody2 body:
                    return new Json { ["kind"] = "body", ["name"] = body.Name };
                case IDimension dimension:
                    return new Json { ["kind"] = "dimension", ["fullName"] = dimension.FullName };
                case ICustomBendAllowance allowance:
                    // swBendAllowanceTypes_e: 1 bend table, 2 K-factor, 3 bend allowance, 4 bend deduction
                    return new Json {
                        ["kind"] = "bendAllowance", ["type"] = allowance.Type, ["kFactor"] = allowance.KFactor,
                        ["bendAllowance"] = allowance.BendAllowance, ["bendDeduction"] = allowance.BendDeduction,
                        ["bendTableFile"] = allowance.BendTableFile
                    };
                case IMathTransform transform:
                    return new Json { ["kind"] = "transform", ["data"] = transform.ArrayData };
                case IMathVector vector:
                    return new Json { ["kind"] = "vector", ["data"] = vector.ArrayData };
                case IMathPoint mathPoint:
                    return new Json { ["kind"] = "point", ["data"] = mathPoint.ArrayData };
            }
        }
        catch (Exception ex) {
            return new Json { ["kind"] = "unreadable", ["error"] = ex.Message };
        }
        // Anything else (a structural member's group, a corner of a corner relief) by what it says it is
        return DescribeUnknown(value);
    }

    /// <summary>A copy in another format, beside the JSON. The source document is not saved.</summary>
    private static string? SaveCopy(ModelDoc2 doc, string fileName)
    {
        int errors = 0, warnings = 0;
        var saved = Try(() => (object)doc.Extension.SaveAs(Path.Combine(outputDirectory, fileName), (int)swSaveAsVersion_e.swSaveAsCurrentVersion,
            (int)(swSaveAsOptions_e.swSaveAsOptions_Silent | swSaveAsOptions_e.swSaveAsOptions_Copy), null, ref errors, ref warnings));
        if (saved is true)
            return fileName;
        Log($"  {fileName} could not be written, error={errors}, warning={warnings}");
        return null;
    }

    private static List<string>? NamesOf(object? features)
    {
        var names = ArrayOf(features).OfType<Feature>().Select(f => f.Name).ToList();
        return names.Count == 0 ? null : names;
    }

    private static object? First(object? value) => value is double[] { Length: > 0 } values ? values[0] : null;

    /// <summary>SolidWorks returns arrays as object, and null in place of an empty one.</summary>
    private static object[] ArrayOf(object? value) => value is Array array ? array.Cast<object>().ToArray() : Array.Empty<object>();

    /// <summary>One value that cannot be read is left out. It does not end the export of the document.</summary>
    private static object? Try(Func<object?> read)
    {
        try {
            return read();
        }
        catch (Exception) {
            return null;
        }
    }

    private static List<Json>? TryList(Func<List<Json>> read)
    {
        try {
            return read();
        }
        catch (Exception) {
            return null;
        }
    }

    private static Json? TryJson(Func<Json?> read, string what)
    {
        try {
            return read();
        }
        catch (Exception ex) {
            Log($"  The {what} could not be read: {ex.Message}");
            return new Json { ["error"] = ex.Message };
        }
    }

    private static IEnumerable<Type> LoadableTypes(Assembly assembly)
    {
        try {
            return assembly.GetTypes();
        }
        catch (ReflectionTypeLoadException ex) {
            return ex.Types.Where(t => t != null)!;
        }
    }

    private static string SafeFileName(string name) => string.Concat(name.Select(c => Path.GetInvalidFileNameChars().Contains(c) ? '_' : c));

    private static string Sha256(string path)
    {
        using var sha = SHA256.Create();
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
        return string.Concat(sha.ComputeHash(stream).Select(b => b.ToString("x2")));
    }

    private static void Log(string message) =>
        File.AppendAllText(Path.Combine(outputDirectory, "export.log"), DateTime.UtcNow.ToString("o") + " " + message + "\r\n");
}
