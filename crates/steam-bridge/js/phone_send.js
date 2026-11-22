// Send step for «Отправить на телефон» (tasks 313, 315). The placeholders in the const line below are filled by
// send_to_phone.rs (or tools/phone/dry-run.mjs). With the live flag false this returns the planned steps only:
// no export, no file, no upload. The live path mirrors stock: clip export (Q1 in module 96513, fnExportClip) or
// screenshot file pick (stock u), then the upload builder hi (module 83433) with the phone bucket fH.
// Progress for the UI is written to globalThis.__fablePhone as {id, step: check | export | upload | sent | ...}. `id` is
// __fableRun, a const declared by send_to_phone.rs (send_expr) and tools/phone/dry-run.mjs. The wrapper there writes the
// final result (sent, failed, unknown, refused, dry) to the same object, so a lost reply can be settled from the page.
const __kind="__KIND__", __clipId=__CLIP_ID__, __app=__APP__, __durMs=__DURATION_MS__, __shotPath=__SHOT_PATH__, __live=__LIVE__;
const __MAX_BYTES=100*1024*1024;
globalThis.__fablePhone={id:__fableRun,step:"check"};
const __ck=await __phoneCheck(__kind==="clip"?"3.9.0":"3.9.1");
if(!__ck.query_ok)return {step:"refused",why:"query",eresult:__ck.eresult};
if(!__ck.found_device)return {step:"refused",why:"no_phone"};
if(!__ck.up_to_date)return {step:"refused",why:"outdated"};
const __gid=new __GameID(String(__app));
let __host="";try{__host=await SteamClient.Auth.GetLocalHostname()}catch(_){}
// Upload (stock dn/hi): ok only when the upload object reached the completed state (6), not on a silent error.
const __send=async(file,media,clipLen)=>{
  globalThis.__fablePhone={id:__fableRun,step:"upload"};
  const __up=__S.hi({dataToUpload:file,eBucket:__S.Enums.$z.fH,eMediaType:media,gameID:__gid,clipLength:clipLen,strMachineName:__host});
  const __res=await __up.result;
  const __p=__up.fileUpload.file_upload_props;
  if(__res.strError||__p.eUploadState!==6)return {step:"failed",why:"upload",message:String(__res.strError||__p.strErrorDescription||("state "+__p.eUploadState))};
  globalThis.__fablePhone={id:__fableRun,step:"sent"};
  return {step:"sent"};
};
if(__kind!=="clip"){
  if(!__live)return {step:"dry",kind:"shot",settings:"no export"};
  if(!__shotPath)return {step:"refused",why:"no_path"};
  const __file=await __pickFile(__shotPath);
  if(!__file||__file.size===0||__file.size>__MAX_BYTES)return {step:"refused",why:"too_big"};
  return await __send(__file,__S.Enums.t2.gl,undefined);
}
const __modB=__findMod("M.bitrate_kbps<=f");
const __B=__req(__modB).B;
const __settings=__B(__durMs/1000,__MAX_BYTES,12000);
if(__settings===null)return {step:"refused",why:"no_preset"};
const __settingsName=Object.keys(__settings).length?(__settings.bitrate_kbps+" kbps"):"default";
if(!__live)return {step:"dry",kind:"clip",clip_id:__clipId,settings:__settingsName,calls:["Q1(create->{clip_id}, {browserContext, settings}) -> fnExportClip -> File","hi({dataToUpload:File, eBucket:fH, eMediaType:_F, gameID, clipLength, strMachineName})"]};
const __fake={create:async()=>({result:__S.Res.R,clipSummary:{clip_id:__clipId}}),cleanup(){}};
globalThis.__fablePhone={id:__fableRun,step:"export"};
const __expRes=await __req(__modB).Q1(__fake,{browserContext:{ownerWindow:window},settings:__settings})(()=>{});
if(__expRes.eResult!==__S.Res.R||!__expRes.file)return {step:"failed",why:"export",eResult:__expRes.eResult};
if(__expRes.file.size===0||__expRes.file.size>__MAX_BYTES)return {step:"refused",why:"too_big"};
return await __send(__expRes.file,__S.Enums.t2._F,__durMs/1000);
